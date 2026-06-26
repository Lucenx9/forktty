//! Terminal appearance and embedded Ghostty configuration regression tests.

use super::*;

#[test]
fn terminal_font_description_uses_system_monospace_defaults() {
    let mut config = config::AppConfig::default();
    config.appearance.font_family = "JetBrains Mono".to_string();
    config.appearance.font_size = 16;

    let description = default_terminal_font_description(&config);

    assert_eq!(description.to_string(), "monospace");
}

#[test]
fn ghostty_config_text_overrides_terminal_font_and_colors() {
    let appearance = ghostty_terminal_appearance_from_text(
        r##"
        font-family = "JetBrains Mono Nerd Font"
        font-size = 15
        scrollback-limit = 10_000_000
        background = 101010
        foreground = #eeeeee
        cursor-color = ffffff
        cursor-text = 000000
        selection-background = 333333
        selection-foreground = f0f0f0
        palette = 0=#111111
        palette = 9=ff4444
        "##,
    );

    assert_eq!(
        appearance.font_family.as_deref(),
        Some("JetBrains Mono Nerd Font")
    );
    assert_eq!(appearance.font_size_pt, Some(15.0));
    assert_eq!(appearance.scrollback_limit_bytes, Some(10_000_000));
    assert_eq!(appearance.colors.background, "#101010");
    assert_eq!(appearance.colors.foreground, "#eeeeee");
    assert_eq!(appearance.colors.cursor, "#ffffff");
    assert_eq!(appearance.colors.cursor_foreground, "#000000");
    assert_eq!(appearance.colors.highlight, "#333333");
    assert_eq!(appearance.colors.highlight_foreground, "#f0f0f0");
    assert_eq!(appearance.colors.ansi[0], "#111111");
    assert_eq!(appearance.colors.ansi[9], "#ff4444");
}

#[test]
fn ghostty_config_text_accumulates_font_family_fallbacks_and_resets() {
    let appearance = ghostty_terminal_appearance_from_text(
        r#"
        font-family = JetBrains Mono
        font-family = Symbols Nerd Font
        "#,
    );

    assert_eq!(
        appearance.font_family.as_deref(),
        Some("JetBrains Mono, Symbols Nerd Font")
    );

    let reset = ghostty_terminal_appearance_from_text(
        r#"
        font-family = JetBrains Mono
        font-family =
        font-family = Menlo
        "#,
    );

    assert_eq!(reset.font_family.as_deref(), Some("Menlo"));
}

#[test]
fn ghostty_config_text_accumulates_styled_font_family_fallbacks() {
    let appearance = ghostty_terminal_appearance_from_text(
        r#"
        font-family-bold = ForkTTYBold
        font-family-bold = Symbols Nerd Font
        font-family-italic = ForkTTYItalic
        font-family-bold-italic =
        font-family-bold-italic = ForkTTYBoldItalic
        "#,
    );

    assert_eq!(
        appearance.font_family_bold.as_deref(),
        Some("ForkTTYBold, Symbols Nerd Font")
    );
    assert_eq!(
        appearance.font_family_italic.as_deref(),
        Some("ForkTTYItalic")
    );
    assert_eq!(
        appearance.font_family_bold_italic.as_deref(),
        Some("ForkTTYBoldItalic")
    );
}

#[test]
fn ghostty_scrollback_limit_overrides_legacy_scrollback_lines() {
    let mut config = config::AppConfig::default();
    config.appearance.scrollback_lines = 777;

    let appearance = ghostty_terminal_appearance_from_text("scrollback-limit = 4096");

    assert_eq!(
        terminal_scrollback_lines_for_appearance(&config, &appearance),
        2
    );
}

#[test]
fn embedded_ghostty_uses_upstream_default_bounded_scrollback() {
    assert_eq!(EMBEDDED_GHOSTTY_SCROLLBACK_LIMIT_BYTES, 10_000_000);
    let appearance = ghostty_terminal_appearance_from_text("");

    assert_eq!(
        embedded_ghostty_scrollback_limit_bytes_for_appearance(&appearance),
        10_000_000
    );
}

#[test]
fn embedded_ghostty_scrollback_limit_follows_ghostty_config() {
    let appearance = ghostty_terminal_appearance_from_text("scrollback-limit = 4096");

    assert_eq!(
        embedded_ghostty_scrollback_limit_bytes_for_appearance(&appearance),
        4096
    );
}

#[test]
fn ghostty_config_loader_resolves_theme_and_recursive_config_files() {
    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    std::fs::write(
        themes.join("User Dark"),
        r##"
        background = #101010
        foreground = #eeeeee
        palette = 0=#000001
        selection-background = #333
        "##,
    )
    .unwrap();
    std::fs::write(
        themes.join("User Light"),
        r##"
        background = #fafafa
        foreground = #111111
        "##,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("included.conf"),
        r##"
        foreground = red
        palette = 9=blue
        config-file = ?missing.conf
        "##,
    )
    .unwrap();
    let config_path = dir.path().join("config.ghostty");
    std::fs::write(
        &config_path,
        r##"
        theme = light:User Light,dark:User Dark
        config-file = included.conf
        background = green
        "##,
    )
    .unwrap();

    let appearance = ghostty_terminal_appearance_from_paths_for_test(
        &[config_path],
        &[themes],
        GhosttyColorScheme::Dark,
    );

    assert_eq!(appearance.colors.background, "#008000");
    assert_eq!(appearance.colors.foreground, "#ff0000");
    assert_eq!(appearance.colors.ansi[0], "#000001");
    assert_eq!(appearance.colors.ansi[9], "#0000ff");
    assert_eq!(appearance.colors.highlight, "#333333");
}

#[test]
fn ghostty_config_loader_ignores_oversized_config_files() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.ghostty");
    let huge_config = format!("background = #123456\n{}", "#".repeat(2 * 1024 * 1024));
    std::fs::write(&config_path, huge_config).unwrap();

    let appearance = ghostty_terminal_appearance_from_paths_for_test(
        &[config_path],
        &[],
        GhosttyColorScheme::Dark,
    );

    assert_eq!(
        appearance.colors.background,
        TerminalColors::forktty_dark().background
    );
}

#[test]
fn ghostty_config_loader_ignores_oversized_theme_files() {
    let dir = tempfile::tempdir().unwrap();
    let themes = dir.path().join("themes");
    std::fs::create_dir_all(&themes).unwrap();
    let huge_theme = format!("background = #123456\n{}", "#".repeat(2 * 1024 * 1024));
    std::fs::write(themes.join("Huge Theme"), huge_theme).unwrap();
    let config_path = dir.path().join("config.ghostty");
    std::fs::write(&config_path, "theme = Huge Theme\n").unwrap();

    let appearance = ghostty_terminal_appearance_from_paths_for_test(
        &[config_path],
        &[themes],
        GhosttyColorScheme::Dark,
    );

    assert_eq!(
        appearance.colors.background,
        TerminalColors::forktty_dark().background
    );
}

#[test]
fn ghostty_config_text_accepts_short_hex_and_named_colors() {
    let appearance = ghostty_terminal_appearance_from_text(
        r##"
        background = #abc
        foreground = blue
        cursor-color = white
        palette = 15=black
        "##,
    );

    assert_eq!(appearance.colors.background, "#aabbcc");
    assert_eq!(appearance.colors.foreground, "#0000ff");
    assert_eq!(appearance.colors.cursor, "#ffffff");
    assert_eq!(appearance.colors.ansi[15], "#000000");
}

#[test]
fn ghostty_config_text_accepts_cell_color_references_and_invert_compat() {
    let appearance = ghostty_terminal_appearance_from_text(
        r##"
        background = #101010
        foreground = #eeeeee
        cursor-color = cell-background
        cursor-text = cell-foreground
        selection-invert-fg-bg
        "##,
    );

    assert_eq!(appearance.colors.cursor, "#101010");
    assert_eq!(appearance.colors.cursor_foreground, "#eeeeee");
    assert_eq!(appearance.colors.highlight, "#eeeeee");
    assert_eq!(appearance.colors.highlight_foreground, "#101010");

    let cursor_invert = ghostty_terminal_appearance_from_text(
        r##"
        background = #202020
        foreground = #dddddd
        cursor-invert-fg-bg = true
        "##,
    );

    assert_eq!(cursor_invert.colors.cursor, "#dddddd");
    assert_eq!(cursor_invert.colors.cursor_foreground, "#202020");
}

#[test]
fn ghostty_config_text_accepts_bold_color() {
    let appearance = ghostty_terminal_appearance_from_text(
        r##"
        foreground = #eeeeee
        bold-color = #ff00aa
        "##,
    );

    assert_eq!(appearance.colors.bold, "#ff00aa");
    assert!(!appearance.colors.bold_is_bright);

    let bright = ghostty_terminal_appearance_from_text("bold-is-bright");
    assert!(bright.colors.bold_is_bright);

    let bright_new_key = ghostty_terminal_appearance_from_text("bold-color = bright");
    assert!(bright_new_key.colors.bold_is_bright);

    let ordered = ghostty_terminal_appearance_from_text(
        r##"
        bold-color = #ff00aa
        foreground = #eeeeee
        "##,
    );
    assert_eq!(ordered.colors.bold, "#ff00aa");
}

#[test]
fn terminal_zoom_font_uses_default_at_reset_and_clamps_steps() {
    let config = config::AppConfig::default();

    assert_eq!(next_terminal_zoom_level(0, 1), 1);
    assert_eq!(next_terminal_zoom_level(1, -1), 0);
    assert_eq!(next_terminal_zoom_level(-99, -1), -6);
    assert_eq!(next_terminal_zoom_level(99, 1), 12);

    let reset = terminal_font_description_for_zoom_level(&config, 0);
    assert_eq!(reset.to_string(), "monospace");

    let zoomed = terminal_font_description_for_zoom_level(&config, 2);
    assert_eq!(zoomed.family().as_deref(), Some("monospace"));
    assert_eq!(zoomed.size(), 14 * gtk::pango::SCALE);

    let smallest = terminal_font_description_for_zoom_level(&config, -99);
    assert_eq!(smallest.size(), 6 * gtk::pango::SCALE);

    let largest = terminal_font_description_for_zoom_level(&config, 99);
    assert_eq!(largest.size(), 24 * gtk::pango::SCALE);
}

#[test]
fn terminal_theme_system_uses_dark_palette() {
    let mut config = config::AppConfig::default();
    config.appearance.terminal_theme = config::TERMINAL_THEME_SYSTEM.to_string();

    assert_eq!(terminal_colors_for_config(&config).background, "#181818");
}

#[test]
fn legacy_terminal_theme_config_is_ignored() {
    let mut config = config::AppConfig::default();
    config.appearance.terminal_theme = config::TERMINAL_THEME_DRACULA.to_string();

    assert_eq!(terminal_colors_for_config(&config).background, "#181818");
}
