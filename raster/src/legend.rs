//! The data layers' legends as the map's panel shows them (`frontend/src/lib/data-legend.json`,
//! written by `qm-raster --legend` and held to it by a test): each layer's colours from the
//! renderer's own colouring, with what they stand for.

use crate::Layer;
use serde_json::{Value, json};

const LAYERS: [(&str, Layer); 8] = [
    ("elevation", Layer::Elevation),
    ("forest", Layer::Forest),
    ("hard", Layer::Hard),
    ("buildings", Layer::Buildings),
    ("traffic", Layer::Traffic),
    ("trains", Layer::Trains),
    ("others", Layer::Others),
    ("barriers", Layer::Barriers),
];

/// A whole number with thousands separated by commas, as the page writes them.
pub fn thousands(value: u32) -> String {
    let digits = value.to_string();
    let mut out = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Every layer's entries: a colour (`#rrggbb`), its opacity and its label.
pub fn legend() -> Value {
    let entries = |layer: Layer| -> Vec<([u8; 4], String)> {
        match layer {
            Layer::Elevation | Layer::Forest | Layer::Hard => crate::ground::legend(layer),
            Layer::Buildings | Layer::Barriers => crate::outlines::legend(layer),
            Layer::Traffic | Layer::Trains | Layer::Others => crate::sources::legend(layer),
        }
    };
    let layers: serde_json::Map<String, Value> = LAYERS
        .iter()
        .map(|&(id, layer)| {
            let rows: Vec<Value> = entries(layer)
                .into_iter()
                .map(|([r, g, b, alpha], label)| {
                    json!({
                        "colour": format!("#{r:02x}{g:02x}{b:02x}"),
                        "opacity": (f64::from(alpha) / 255.0 * 100.0).round() / 100.0,
                        "label": label,
                    })
                })
                .collect();
            (id.to_owned(), Value::Array(rows))
        })
        .collect();
    Value::Object(layers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panels_legend_is_the_renderers() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../frontend/src/lib/data-legend.json");
        let text = std::fs::read_to_string(&path).expect("the panel's legend");
        let panel: Value = serde_json::from_str(&text).expect("JSON");
        assert_eq!(
            panel,
            legend(),
            "{} differs from the renderer's colours: `qm-raster --legend > {}`",
            path.display(),
            path.display()
        );
    }

    #[test]
    fn thousands_are_separated() {
        assert_eq!(thousands(300), "300");
        assert_eq!(thousands(1_800), "1,800");
        assert_eq!(thousands(100_000), "100,000");
    }
}
