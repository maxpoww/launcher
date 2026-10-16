//! Every WGSL shader parses and validates: a shader error only shows when the
//! daemon starts, and then it dies (ASUS, 2026-10-16: a name used twice).

#[test]
fn every_shader_validates() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shaders");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).expect("the shaders' folder") {
        let path = entry.expect("an entry").path();
        if path.extension().and_then(|e| e.to_str()) != Some("wgsl") {
            continue;
        }
        let text = std::fs::read_to_string(&path).expect("a shader reads");
        let module = naga::front::wgsl::parse_str(&text)
            .unwrap_or_else(|e| panic!("{}: {}", path.display(), e.emit_to_string(&text)));
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}: {e:?}", path.display()));
        seen += 1;
    }
    assert!(seen > 3, "only {seen} shaders found");
}
