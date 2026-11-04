//! Build an isolated rail-only runtime tree through the unchanged production source converter.
use crate::{
    dev4::{Dev4, Square},
    source_tables::SourceTables,
    sources,
    year::{Kind, YearInputs},
};
use std::{collections::BTreeMap, path::PathBuf};

#[test]
#[ignore = "requires an explicit fixture plan, prepared rails and retained propagation inputs"]
fn build_rail_ruler_tree() {
    let plan: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("QM_RAIL_RULER_FIXTURE").unwrap()).unwrap(),
    )
    .unwrap();
    let path = |key: &str| PathBuf::from(plan[key].as_str().unwrap());
    let inputs = YearInputs::read(&path("manifest"), &path("sources")).unwrap();
    let tables = SourceTables::read(&inputs).unwrap();
    let network =
        sources::tent::FreightNetwork::load(inputs.required(Kind::Tent).unwrap()).unwrap();
    let dev4 = Dev4 {
        prepared: path("prepared"),
        rasters: PathBuf::new(),
    };
    let output = path("output");
    let terrain = path("terrain");
    let squares = crate::parse_squares(plan["squares"].as_str().unwrap()).unwrap();
    for square in squares {
        let decks = sources::decks::Decks::of_square(&dev4, square, &terrain).unwrap();
        let mut converted = Vec::new();
        for neighbour in square.with_neighbours() {
            sources::rail::convert(
                (&dev4, &network, &tables),
                neighbour,
                &decks,
                &mut converted,
            )
            .unwrap();
        }
        let mut by_tile = BTreeMap::<_, Vec<_>>::new();
        for piece in converted
            .into_iter()
            .filter(|piece| Square::of(piece.tile) == square)
        {
            by_tile.entry(piece.tile).or_default().push(piece);
        }
        let mut writer =
            tiles::square::SquareWriter::create(&output, tiles::Kind::Sources, square).unwrap();
        for tile in square.tiles() {
            let Some(converted) = by_tile.remove(&tile) else {
                continue;
            };
            let mut pieces = Vec::new();
            let mut attributes = Vec::new();
            let mut decks = Vec::new();
            for piece in converted {
                if let Some(altitude_m) = piece.deck_m {
                    decks.push(tiles::sources::Deck {
                        piece: pieces.len() as u32,
                        altitude_m,
                    });
                }
                pieces.push(tiles::sources::Piece {
                    ends: piece.ends,
                    attribute: attributes.len() as u32,
                });
                attributes.push(piece.attribute);
            }
            writer
                .add(tile, &tiles::sources::encode(&pieces, &attributes, &decks))
                .unwrap();
        }
        writer.finish().unwrap();
    }
    tiles::mark_year_complete(
        &output,
        &std::fs::read(&inputs.manifest_path).unwrap(),
        "Isolated railway ruler fixture; other emissions intentionally absent",
    )
    .unwrap();
}
