//! Match normalized observations through one topology and emit category claims on original way intervals.
use super::{
    leg_matcher::LegMatcher,
    model::RailModel,
    movements::{self, Location, Movement, Record},
    network::Network,
    service,
    source_routes::Routes,
    square_intervals::Interval,
    types::Crossing,
};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
};

pub type Claims = HashMap<[u16; 2], Vec<(Interval, i64)>>;

pub struct MatchedInputs {
    pub claims: Claims,
    pub crossings: HashMap<[u16; 2], Vec<Crossing>>,
}

pub(super) fn interval(
    movement: &Movement,
    way: i64,
    segment: i16,
    from: f64,
    to: f64,
    matching: u8,
) -> Interval {
    Interval {
        osm_id: way,
        segment_idx: segment,
        from_m: from.min(to),
        to_m: from.max(to),
        country: movement.country.as_bytes().try_into().unwrap(),
        source_id: movement.source_id,
        passenger: movement.passenger.unwrap_or(0.0) + movement.missing_calendar_estimate[0],
        freight: movement.freight.unwrap_or(0.0) + movement.missing_calendar_estimate[1],
        passenger_status: movement.passenger_status,
        freight_status: movement.freight_status,
        matching,
        missing_calendar_estimate: movement.missing_calendar_estimate,
        stop: 0,
        resolved_section: None,
    }
}

fn expand_halo(squares: &HashSet<[u16; 2]>) -> HashSet<[u16; 2]> {
    let mut expanded = HashSet::with_capacity(squares.len());
    for &[x, y] in squares {
        for dx in -1i32..=1 {
            for dy in -1i32..=1 {
                let (nx, ny) = (i32::from(x) + dx, i32::from(y) + dy);
                if (0..512).contains(&nx) && (0..512).contains(&ny) {
                    expanded.insert([nx as u16, ny as u16]);
                }
            }
        }
    }
    expanded
}

fn bounds_squares(bounds: [f64; 4]) -> [u16; 4] {
    let nw = crate::osm::geometry::square_of(bounds[2], bounds[1]);
    let se = crate::osm::geometry::square_of(bounds[0], bounds[3]);
    [nw.x, se.x, nw.y, se.y]
}

fn intersects(bounds: [f64; 4], squares: &HashSet<[u16; 2]>) -> bool {
    let [x0, x1, y0, y1] = bounds_squares(bounds);
    squares
        .iter()
        .any(|square| (x0..=x1).contains(&square[0]) && (y0..=y1).contains(&square[1]))
}

pub fn read(
    model: &RailModel,
    geography: &Path,
    flows: &Path,
    start: &str,
    end: &str,
    squares: &HashSet<[u16; 2]>,
) -> Result<MatchedInputs, String> {
    let mut groups = BTreeMap::<(u16, String), Vec<Movement>>::new();
    let mut crossing_groups = BTreeMap::<u16, Vec<Crossing>>::new();
    let mut result = MatchedInputs {
        claims: HashMap::new(),
        crossings: HashMap::new(),
    };
    let halo_squares = expand_halo(squares);
    let mut interval_keys: Vec<super::yards::YardEdgeKey> = Vec::new();
    let mut unlocated = (0usize, 0.0, 0.0);
    let mut incomplete_zeros = 0;
    let mut stops = 0;
    movements::read(flows, start, end, |record| {
        match record {
            Record::Movement(movement) => {
                let mut movement = *movement;
                if model.source(movement.source_id).is_none() {
                    return Err(format!(
                        "unknown rail movement source {}",
                        movement.source_id
                    ));
                }
                if matches!(movement.location, Location::Unlocated { .. }) {
                    unlocated.0 += 1;
                    unlocated.1 +=
                        movement.passenger.unwrap_or(0.0) + movement.freight.unwrap_or(0.0);
                    unlocated.2 += movement.missing_calendar_estimate.iter().sum::<f64>();
                    return Ok(());
                }
                if !movement.retain_categories_with_annual_evidence() {
                    incomplete_zeros += 1;
                    return Ok(());
                }
                if let Location::Interval {
                    square,
                    way,
                    segment,
                    from_m,
                    to_m,
                    occurrence,
                } = movement.location
                {
                    interval_keys.push((square, way, segment));
                    if halo_squares.contains(&square) {
                        result.claims.entry(square).or_default().push((
                            interval(&movement, way, segment, from_m, to_m, movement.matching),
                            occurrence,
                        ));
                    }
                } else {
                    groups
                        .entry((movement.source_id, movement.country.clone()))
                        .or_default()
                        .push(movement);
                }
            }
            Record::Crossing(crossing) => {
                if model.source(crossing.source_id).is_none() {
                    return Err(format!(
                        "unknown rail crossing source {}",
                        crossing.source_id
                    ));
                }
                if crossing.soundings.iter().any(|&soundings| soundings > 0.0) {
                    let square = crate::osm::geometry::square_of(crossing.lat, crossing.lon);
                    result
                        .crossings
                        .entry([square.x, square.y])
                        .or_default()
                        .push(crossing.clone());
                }
                if crossing.total_periods.is_some() {
                    crossing_groups
                        .entry(crossing.source_id)
                        .or_default()
                        .push(crossing);
                }
            }
            Record::Window { .. } => unreachable!(),
        }
        Ok(())
    })?;
    // When crossing inventories allocate across global cross-sections, keep 1-ring neighbour
    // timetable claims so boundary tracks see their parallel sibling's timetable across cuts,
    // and track direct-claim keys across the full crossing graph bounds for yard exemptions.
    let claim_squares = if crossing_groups.is_empty() {
        squares
    } else {
        &halo_squares
    };
    let mut yard_bounds: Vec<[u16; 4]> = Vec::new();
    for &source_id in crossing_groups.keys() {
        let source = model
            .source(source_id)
            .ok_or_else(|| format!("unknown rail crossing source {source_id}"))?;
        let scope = format!("{source_id}:{}", source.owner);
        if let Some(&bounds) = model.network_bounds.get(&scope)
            && intersects(bounds, squares)
        {
            yard_bounds.push(bounds_squares(bounds));
        }
    }
    let in_yard_bounds = |sq: [u16; 2]| {
        yard_bounds
            .iter()
            .any(|b| sq[0] >= b[0] && sq[0] <= b[1] && sq[1] >= b[2] && sq[1] <= b[3])
    };
    let mut yard_direct_claims: HashSet<super::yards::YardEdgeKey> = interval_keys
        .into_iter()
        .filter(|&(sq, _, _)| in_yard_bounds(sq))
        .collect();
    if unlocated.0 > 0 {
        eprintln!(
            "railways: {} unlocated observations: {} observed and {} calendar-estimated daily departures remain spatially unknown",
            unlocated.0, unlocated.1, unlocated.2
        );
    }
    eprintln!(
        "railways: {incomplete_zeros} zero-only partial-calendar observations retain modelled traffic"
    );
    let routes = if groups
        .values()
        .flatten()
        .any(|movement| matches!(movement.location, Location::Service { .. }))
    {
        Some(Routes::read(geography)?)
    } else {
        None
    };
    let rail_squares = if groups
        .values()
        .flatten()
        .any(super::local_match::observed_locally)
    {
        super::local_match::rail_squares(geography, claim_squares)?
    } else {
        Vec::new()
    };
    for ((source, country), movements) in groups {
        let scope = format!("{source}:{country}");
        let bounds = *model
            .network_bounds
            .get(&scope)
            .ok_or_else(|| format!("rail model lacks network bounds for {scope}"))?;
        if !intersects(bounds, claim_squares) {
            continue;
        }
        eprintln!("railways: match {scope}: {} observations", movements.len());
        let services = movements
            .iter()
            .filter(|movement| matches!(movement.location, Location::Service { .. }));
        if services.clone().next().is_some() {
            let graph = Network::read(geography, bounds)?;
            let mut matcher = LegMatcher::new(&graph);
            let mut occurrence = 0;
            let mut totals = [0; 4];
            for (index, movement) in services.enumerate() {
                let Location::Service { stops, shape } = &movement.location else {
                    unreachable!()
                };
                let matched =
                    service::match_service(routes.as_ref().unwrap(), &mut matcher, stops, shape);
                totals[usize::from(matched.matching)] += 1;
                totals[3] += matched.failed;
                let offset = occurrence;
                for passage in matched.passages {
                    occurrence = occurrence.max(offset + passage.occurrence + 1);
                    if in_yard_bounds(passage.square) {
                        yard_direct_claims.insert((passage.square, passage.way, passage.segment));
                    }
                    if claim_squares.contains(&passage.square) {
                        result.claims.entry(passage.square).or_default().push((
                            interval(
                                movement,
                                passage.way,
                                passage.segment,
                                passage.from,
                                passage.to,
                                matched.matching,
                            ),
                            offset + passage.occurrence,
                        ));
                    }
                }
                if index % 1000 == 0 {
                    eprintln!(
                        "railways: {scope}: {index} patterns, {} claims",
                        result.claims.values().map(Vec::len).sum::<usize>()
                    );
                }
            }
            matcher.report();
            eprintln!(
                "railways: {scope}: {} relation, {} graph, {} failed legs",
                totals[1], totals[2], totals[3]
            );
        }
        if movements
            .iter()
            .any(|movement| matches!(movement.location, Location::StationPair { .. }))
        {
            let graph = Network::read(geography, bounds)?;
            let pairs = super::station_pairs::match_pairs(&graph, &movements);
            let template = movements
                .iter()
                .find(|movement| matches!(movement.location, Location::StationPair { .. }))
                .unwrap();
            for (index, edge) in graph.edges.iter().enumerate() {
                if edge.traversal_only {
                    continue;
                }
                let observed = pairs.edges.get(&index);
                let residual = model
                    .residuals
                    .get(&scope)
                    .filter(|_| !pairs.quarantine.contains(&index));
                if observed.is_none() && residual.is_none() {
                    continue;
                }
                if in_yard_bounds(edge.square) {
                    yard_direct_claims.insert((edge.square, edge.way, edge.segment));
                }
                if !claim_squares.contains(&edge.square) {
                    continue;
                }
                let movement = if let Some(observed) = observed {
                    observed.clone()
                } else {
                    let residual = residual.unwrap();
                    let mut row = template.clone();
                    row.source_id = residual.source_id;
                    row.passenger = Some(residual.passenger);
                    row.freight = Some(residual.freight);
                    row.passenger_status = 2;
                    row.freight_status = 2;
                    row.coverage = Default::default();
                    row.estimate_missing_calendar();
                    row
                };
                result.claims.entry(edge.square).or_default().push((
                    interval(
                        &movement,
                        edge.way,
                        edge.segment,
                        edge.extent[0],
                        edge.extent[1],
                        if observed.is_some() { 2 } else { 0 },
                    ),
                    0,
                ));
            }
        }
        stops = super::local_match::apply(
            model,
            geography,
            &movements,
            &rail_squares,
            &mut result.claims,
            stops,
        )?;
    }
    if !yard_bounds.is_empty() {
        for (&square, list) in &result.claims {
            if in_yard_bounds(square) {
                for (iv, _) in list {
                    if iv.resolved_section.is_none() {
                        yard_direct_claims.insert((square, iv.osm_id, iv.segment_idx));
                    }
                }
            }
        }
    }
    for (source_id, crossings) in crossing_groups {
        let source = model
            .source(source_id)
            .ok_or_else(|| format!("unknown rail crossing source {source_id}"))?;
        let country: [u8; 2] = source
            .owner
            .as_bytes()
            .try_into()
            .map_err(|_| format!("rail crossing source {source_id} lacks a 2-letter owner"))?;
        let scope = format!("{source_id}:{}", source.owner);
        let bounds = *model
            .network_bounds
            .get(&scope)
            .ok_or_else(|| format!("rail model lacks network bounds for {scope}"))?;
        if !intersects(bounds, squares) {
            continue;
        }
        eprintln!(
            "railways: match {scope}: {} flow crossings",
            crossings.len()
        );
        let graph = Network::read(geography, bounds)?;
        let graph_squares: HashSet<[u16; 2]> = graph
            .edges
            .iter()
            .filter(|edge| !edge.traversal_only && edge.rail_type == 0 && edge.country == country)
            .map(|edge| edge.square)
            .collect();
        let yard_keys =
            super::yards::yard_stamped_keys(geography, &graph_squares, &yard_direct_claims)?;
        for (square, claim) in super::crossings::match_crossings(
            model,
            &graph,
            &crossings,
            source_id,
            country,
            squares,
            &result.claims,
            &yard_keys,
        ) {
            result.claims.entry(square).or_default().push((claim, 0));
        }
    }
    result.claims.retain(|square, _| squares.contains(square));
    Ok(result)
}
