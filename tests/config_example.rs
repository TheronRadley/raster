use raster::config::Config;
use raster::core::commands::CommandId;
use raster::term::input::KeyChord;

#[test]
fn shipped_example_config_is_valid_and_overrides_are_resolvable() {
    let config = Config::from_toml(include_str!("../examples/config.toml"))
        .expect("examples/config.toml must be valid TOML for Raster's parser");
    assert_eq!(config.lsp.servers.len(), 1);
    assert_eq!(config.lsp.servers[0].command, "rust-analyzer");
    assert!(!config.lsp.enabled);
    assert!(!config.clipboard.osc52);
    assert_eq!(config.editor.newline, raster::config::NewlineMode::Preserve);
    let map = config.key_map().expect("example key overrides must parse");
    assert_eq!(map[&KeyChord::parse("ctrl+s").unwrap()], CommandId::Save);
    assert_eq!(
        map[&KeyChord::parse("ctrl+shift+p").unwrap()],
        CommandId::CommandPalette
    );
}
