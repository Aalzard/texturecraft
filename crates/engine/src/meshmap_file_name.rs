//! Which texture set and mesh map a baked map file is for, by file name (`Body_ao.1001.png` = Body's ambient occlusion)

use std::path::Path;

use texturecraft_core::{BakedMap, UDIM_RANGE};

/// Mesh maps named by two words anywhere in the file name; they win over the one-word names
const TWO_WORD_NAMES: [(&str, &[&str], BakedMap); 3] = [
    ("world", &["normal", "normals"], BakedMap::WorldNormal),
    ("bent", &["normal", "normals"], BakedMap::BentNormal),
    ("ambient", &["occlusion"], BakedMap::AmbientOcclusion),
];

const ONE_WORD_NAMES: [(&str, BakedMap); 20] = [
    ("worldnormal", BakedMap::WorldNormal),
    ("worldnormals", BakedMap::WorldNormal),
    ("bentnormal", BakedMap::BentNormal),
    ("bentnormals", BakedMap::BentNormal),
    ("ambientocclusion", BakedMap::AmbientOcclusion),
    ("normal", BakedMap::Normal),
    ("normals", BakedMap::Normal),
    ("nrm", BakedMap::Normal),
    ("occlusion", BakedMap::AmbientOcclusion),
    ("ao", BakedMap::AmbientOcclusion),
    ("curvature", BakedMap::Curvature),
    ("curv", BakedMap::Curvature),
    ("id", BakedMap::Id),
    ("position", BakedMap::Position),
    ("pos", BakedMap::Position),
    ("thickness", BakedMap::Thickness),
    ("height", BakedMap::Height),
    ("displacement", BakedMap::Height),
    ("opacity", BakedMap::Opacity),
    ("alpha", BakedMap::Opacity),
];

fn is_separator(c: char) -> bool {
    matches!(c, '_' | '-' | '.' | ' ')
}

/// The file name without its folder and extension
fn stem(path: &str) -> &str {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or_default()
}

/// The lowercase words of a file name, without UV tile numbers (`Body_AO.1001` → body, ao)
fn words(path: &str) -> Vec<String> {
    stem(path)
        .split(is_separator)
        .filter(|w| !w.is_empty() && !w.parse::<u32>().is_ok_and(|n| w.len() == 4 && UDIM_RANGE.contains(&n)))
        .map(str::to_lowercase)
        .collect()
}

/// The mesh map a file holds, from the words of its name; the last recognised word wins
pub(crate) fn baked_map_from_file_name(path: &str) -> Option<BakedMap> {
    let words = words(path);
    let has = |word: &str| words.iter().any(|w| w == word);
    TWO_WORD_NAMES
        .iter()
        .find(|(first, seconds, _)| has(first) && seconds.iter().any(|s| has(s)))
        .map(|(_, _, map)| *map)
        .or_else(|| words.iter().rev().find_map(|w| ONE_WORD_NAMES.iter().find(|(name, _)| name == w).map(|(_, map)| *map)))
}

/// The set a file is for: the longest set name its name starts with before a separator (any name with one set)
pub(crate) fn texture_set_from_file_name(path: &str, set_names: &[&str]) -> Option<usize> {
    if set_names.len() == 1 {
        return Some(0);
    }

    let stem = stem(path).to_lowercase();
    set_names
        .iter()
        .enumerate()
        .filter(|(_, name)| !name.is_empty())
        .filter(|(_, name)| stem.strip_prefix(&name.to_lowercase()).and_then(|rest| rest.chars().next()).is_some_and(is_separator))
        .max_by_key(|(_, name)| name.chars().count())
        .map(|(ts, _)| ts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_word_names_its_map() {
        let cases = [
            ("Body_normal.png", BakedMap::Normal),
            ("Body_normals.png", BakedMap::Normal),
            ("Body_nrm.png", BakedMap::Normal),
            ("Body_world_normal.png", BakedMap::WorldNormal),
            ("Body_normals_world.png", BakedMap::WorldNormal),
            ("Body_WorldNormal.png", BakedMap::WorldNormal),
            ("Body_bent_normals.png", BakedMap::BentNormal),
            ("Body-ambient-occlusion.png", BakedMap::AmbientOcclusion),
            ("Body_AmbientOcclusion.png", BakedMap::AmbientOcclusion),
            ("Body_occlusion.png", BakedMap::AmbientOcclusion),
            ("Body AO.png", BakedMap::AmbientOcclusion),
            ("Body_curvature.png", BakedMap::Curvature),
            ("Body_curv.png", BakedMap::Curvature),
            ("Body_ID.png", BakedMap::Id),
            ("Body_position.png", BakedMap::Position),
            ("Body_pos.png", BakedMap::Position),
            ("Body_thickness.png", BakedMap::Thickness),
            ("Body_height.png", BakedMap::Height),
            ("Body_displacement.png", BakedMap::Height),
            ("Body_opacity.png", BakedMap::Opacity),
            ("Body_alpha.png", BakedMap::Opacity),
        ];
        for (file, map) in cases {
            assert_eq!(baked_map_from_file_name(file), Some(map), "{file}");
        }
    }

    #[test]
    fn tile_numbers_and_folders_are_ignored() {
        assert_eq!(baked_map_from_file_name("/bakes/Body_ao.1001.png"), Some(BakedMap::AmbientOcclusion));
        assert_eq!(baked_map_from_file_name("Body_normal_1002.tga"), Some(BakedMap::Normal));
        assert_eq!(texture_set_from_file_name("/bakes/Body_ao.1001.png", &["Head", "Body"]), Some(1));
    }

    #[test]
    fn unknown_words_match_no_map() {
        assert_eq!(baked_map_from_file_name("Body_diffuse.png"), None);
        assert_eq!(baked_map_from_file_name("Body_1001.png"), None);
        assert_eq!(baked_map_from_file_name("ambient.png"), None);
        assert_eq!(baked_map_from_file_name(""), None);
    }

    #[test]
    fn the_longest_set_name_wins() {
        let sets = ["Body", "Body_Upper", "Head"];
        assert_eq!(texture_set_from_file_name("Body_Upper_ao.png", &sets), Some(1));
        assert_eq!(texture_set_from_file_name("body_ao.png", &sets), Some(0));
        assert_eq!(texture_set_from_file_name("HEAD-normal.png", &sets), Some(2));
    }

    #[test]
    fn a_set_name_must_end_at_a_separator() {
        let sets = ["Body", "Head"];
        assert_eq!(texture_set_from_file_name("Bodywork_ao.png", &sets), None);
        assert_eq!(texture_set_from_file_name("Arm_ao.png", &sets), None);
        assert_eq!(texture_set_from_file_name("Body.png", &sets), None);
    }

    #[test]
    fn one_set_takes_every_file() {
        assert_eq!(texture_set_from_file_name("ao.png", &["Body"]), Some(0));
        assert_eq!(texture_set_from_file_name("Head_ao.png", &["Body"]), Some(0));
        assert_eq!(texture_set_from_file_name("ao.png", &[]), None);
    }
}
