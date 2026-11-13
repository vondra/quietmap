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
    }
}

fn intersects(bounds: [f64; 4], squares: &HashSet<[u16; 2]>) -> bool {
    let nw = crate::osm::geometry::square_of(bounds[2], bounds[1]);
    let se = crate::osm::geometry::square_of(bounds[0], bounds[3]);
    squares
        .iter()
        .any(|square| (nw.x..=se.x).contains(&square[0]) && (nw.y..=se.y).contains(&square[1]))
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
    let mut result = MatchedInputs {
        claims: HashMap::new(),
        crossings: HashMap::new(),
    };
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
                    if squares.contains(&square) {
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
                let square = crate::osm::geometry::square_of(crossing.lat, crossing.lon);
                result
                    .crossings
                    .entry([square.x, square.y])
                    .or_default()
                    .push(crossing);
            }
            Record::Window { .. } => unreachable!(),
        }
        Ok(())
    })?;
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
        super::local_match::rail_squares(geography, squares)?
    } else {
        Vec::new()
    };
    for ((source, country), movements) in groups {
        let scope = format!("{source}:{country}");
        let bounds = *model
            .network_bounds
            .get(&scope)
            .ok_or_else(|| format!("rail model lacks network bounds for {scope}"))?;
        if !intersects(bounds, squares) {
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
                    if squares.contains(&passage.square) {
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
                if !squares.contains(&edge.square) || edge.traversal_only {
                    continue;
                }
                let observed = pairs.edges.get(&index);
                let residual = model
                    .residuals
                    .get(&scope)
                    .filter(|_| !pairs.quarantine.contains(&index));
                let movement = if let Some(observed) = observed {
                    observed.clone()
                } else if let Some(residual) = residual {
                    let mut row = template.clone();
                    row.source_id = residual.source_id;
                    row.passenger = Some(residual.passenger);
                    row.freight = Some(residual.freight);
                    row.passenger_status = 2;
                    row.freight_status = 2;
                    row.coverage = Default::default();
                    row.estimate_missing_calendar();
                    row
                } else {
                    continue;
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
    Ok(result)
}
