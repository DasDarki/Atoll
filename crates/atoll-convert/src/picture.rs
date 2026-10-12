use std::path::Path;

use image::ImageReader;

use crate::{ConvertError, Recipe};

/// Converts a still picture without any tool from outside.
pub(crate) fn convert(source: &Path, target: &Path, recipe: &Recipe) -> Result<(), ConvertError> {
    let reader = ImageReader::open(source)
        .map_err(|error| ConvertError::Unreadable(error.to_string()))?
        .with_guessed_format()
        .map_err(|error| ConvertError::Unreadable(error.to_string()))?;
    let picture = reader
        .decode()
        .map_err(|error| ConvertError::Unreadable(error.to_string()))?;
    let picture = if recipe.mirror {
        picture.fliph()
    } else {
        picture
    };
    // JPEG knows no transparency, so what was see-through would turn black.
    let wants_jpeg = target
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"));
    let picture = if wants_jpeg {
        image::DynamicImage::ImageRgb8(picture.to_rgb8())
    } else {
        picture
    };
    picture
        .save(target)
        .map_err(|error| ConvertError::Unwritable(error.to_string()))
}
