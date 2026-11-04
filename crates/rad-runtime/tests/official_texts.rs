//! Official plugins speak the user's language: every text of theirs that the editor shows
//! has a translation in each catalog the editor embeds.
use std::{fs, path::PathBuf};

#[test]
fn every_text_an_official_plugin_shows_has_a_spanish_translation() {
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    for name in ["audio", "media", "metrics", "obs"] {
        let bytes = fs::read(repo.join(format!("plugins/{name}/manifest.json"))).unwrap();
        let manifest = rad_plugin_contract::Document::new(&bytes)
            .unwrap()
            .manifest(rad_plugin_contract::HOST_API, "x86_64-unknown-linux-gnu")
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        let mut texts = vec![];
        for definition in manifest.actions.iter().chain(&manifest.visuals) {
            texts.push(definition.label.as_str());
            for field in &definition.schema.fields {
                texts.push(&field.label);
                if let rad_plugin_contract::FieldKind::Choice { options } = &field.kind {
                    texts.extend(options.iter().map(|o| o.label.as_str()));
                }
            }
        }
        for field in manifest.settings.iter().flat_map(|s| &s.fields) {
            texts.push(&field.label);
        }
        for text in texts {
            // Names that read the same in Spanish, like "CPU total", need no entry.
            let same = ["CPU total"].contains(&text);
            assert!(
                same || manifest.translate("es", text) != text,
                "{name}: «{text}» has no Spanish translation"
            );
        }
    }
}

#[test]
fn binary_official_manifests_fit_wire_budget_and_translate_all_state_labels() {
    for bytes in [
        include_bytes!("../../../plugins/audio/manifest.json").as_slice(),
        include_bytes!("../../../plugins/obs/manifest.json").as_slice(),
    ] {
        assert!(bytes.len() <= 65536);
        let manifest = rad_plugin_contract::Document::new(bytes)
            .unwrap()
            .manifest(1, "x86_64-unknown-linux-gnu")
            .unwrap();
        assert_eq!(manifest.version.minor, 4);
        for action in &manifest.actions {
            assert!(action.visual.is_some());
        }
        for visual in &manifest.visuals {
            assert_eq!(visual.states.len(), 2);
            for state in &visual.states {
                // Hex uses two characters per byte; official PNG icons have a 4 KiB budget.
                assert!(state.icon.as_ref().is_some_and(|i| i.len() <= 2 * 4 * 1024));
                assert!(manifest.translations["es"].contains_key(&state.label));
            }
        }
    }
}
