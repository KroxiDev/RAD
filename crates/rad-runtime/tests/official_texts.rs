//! Official two-state plugins fit the manifest wire budget and translate every state label.

#[test]
fn binary_official_manifests_fit_wire_budget_and_translate_all_state_labels() {
    for bytes in [
        include_bytes!("../../../plugins/audio/manifest.json").as_slice(),
        include_bytes!("../../../plugins/obs/manifest.json").as_slice(),
    ] {
        assert!(bytes.len() <= 65536);
        let manifest = rad_plugin_contract::Document::new(bytes)
            .unwrap()
            .manifest(rad_plugin_contract::HOST_API, rad_plugin_host::TARGET)
            .unwrap();
        // La identidad y versión se contrastan con el proceso en rad-plugin-check;
        // estos presupuestos y traducciones se conservan al publicar otra versión.
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
