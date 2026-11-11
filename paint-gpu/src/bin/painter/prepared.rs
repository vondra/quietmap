//! A parsed square and its backing files retained together across preparation and painting.

use paint::square::{Files, Square};
use physics::weather::WeatherTable;
use tiles::geo::TileId;

struct SquareFiles<'a> {
    files: Files,
    weather: &'a WeatherTable,
    tile: TileId,
}

self_cell::self_cell!(
    pub(super) struct PreparedSquare<'a> {
        owner: SquareFiles<'a>,
        #[covariant]
        dependent: Square,
    }
);

impl<'a> PreparedSquare<'a> {
    pub fn from_files(
        files: Files,
        weather: &'a WeatherTable,
        tile: TileId,
    ) -> Result<Self, String> {
        Self::try_new(
            SquareFiles {
                files,
                weather,
                tile,
            },
            |owner| Square::new(owner.weather, owner.tile, &owner.files),
        )
    }

    pub fn files(&self) -> &Files {
        &self.borrow_owner().files
    }
}
