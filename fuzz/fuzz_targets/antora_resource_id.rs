//! `parse_resource_id` returns for any text, every id it accepts keeps Antora's rules (a
//! relative path of named segments, coordinates that are not empty and hold no separator),
//! and writing an accepted id out with `resource_id_string`, as the MCP server does, and
//! parsing that again gives the same id.

#![no_main]

use adoc_antora::parse_resource_id;
use adoc_engine::resource_id_string;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let input = String::from_utf8_lossy(data);
    let Ok(id) = parse_resource_id(&input) else {
        return;
    };

    assert!(
        !id.path.is_empty()
            && !id.path.contains(['\\', '#'])
            && id
                .path
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != ".."),
        "{input:?} gave the path {:?}",
        id.path
    );
    if let Some(version) = &id.version {
        assert!(
            !version.is_empty() && !version.contains([':', '$', '@']),
            "{input:?} gave the version {version:?}"
        );
    }
    for coordinate in [&id.component, &id.module].into_iter().flatten() {
        assert!(
            !coordinate.is_empty() && !coordinate.contains([':', '$', '@']),
            "{input:?} gave the coordinate {coordinate:?}"
        );
    }
    assert!(
        id.component.is_none() || id.module.is_some(),
        "{input:?} gave a component without a module"
    );

    let written = resource_id_string(&id);
    assert_eq!(
        parse_resource_id(&written).as_ref(),
        Ok(&id),
        "{input:?} was written as {written:?}"
    );
});
