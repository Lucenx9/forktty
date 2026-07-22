//! Public stylesheet contracts for ForkTTY's GTK 4.14 design system.

use std::collections::BTreeSet;

const CSS: &str = include_str!("../../style.css");

fn block(selector: &str) -> &str {
    CSS.split(selector)
        .nth(1)
        .and_then(|rest| rest.split('}').next())
        .unwrap_or_else(|| panic!("missing CSS block {selector}"))
}

fn without_comments(source: &str) -> String {
    let mut remaining = source;
    let mut result = String::with_capacity(source.len());
    while let Some(start) = remaining.find("/*") {
        result.push_str(&remaining[..start]);
        let comment = &remaining[start + 2..];
        let end = comment.find("*/").expect("unterminated CSS comment");
        remaining = &comment[end + 2..];
    }
    result.push_str(remaining);
    result
}

fn validate_declarations(
    source: &str,
    property: &str,
    allowed: &[&str],
) -> Result<BTreeSet<String>, String> {
    let source = without_comments(source);
    let mut values = BTreeSet::new();
    for (line_number, line) in source.lines().enumerate() {
        let line = line.rsplit_once('{').map_or(line, |(_, rest)| rest).trim();
        let Some(rest) = line.strip_prefix(property) else {
            continue;
        };
        let rest = rest.trim_start();
        if rest.starts_with('-') {
            continue;
        }
        let Some(rest) = rest.strip_prefix(':') else {
            return Err(format!(
                "malformed {property} declaration on line {}",
                line_number + 1
            ));
        };
        let value = rest.trim();
        let Some(value) = value.strip_suffix(';') else {
            return Err(format!(
                "{property} declaration on line {} must end with a semicolon",
                line_number + 1
            ));
        };
        let components = value.split_whitespace().collect::<Vec<_>>();
        if components.is_empty() {
            return Err(format!(
                "empty {property} declaration on line {}",
                line_number + 1
            ));
        }
        for component in components {
            if !allowed.contains(&component) {
                return Err(format!(
                    "noncanonical {property} on line {}: {component}",
                    line_number + 1
                ));
            }
            values.insert(component.to_string());
        }
    }
    Ok(values)
}

fn defined_hex_color(name: &str) -> [u8; 3] {
    let prefix = format!("@define-color {name} #");
    let value = CSS
        .lines()
        .find_map(|line| line.trim().strip_prefix(&prefix))
        .and_then(|rest| rest.strip_suffix(';'))
        .unwrap_or_else(|| panic!("missing hex definition for @{name}"));
    assert_eq!(value.len(), 6, "@{name} must use a six-digit hex color");
    [
        u8::from_str_radix(&value[0..2], 16).unwrap(),
        u8::from_str_radix(&value[2..4], 16).unwrap(),
        u8::from_str_radix(&value[4..6], 16).unwrap(),
    ]
}

fn relative_luminance(rgb: [u8; 3]) -> f64 {
    let linear = |channel: u8| {
        let value = f64::from(channel) / 255.0;
        if value <= 0.04045 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(rgb[0]) + 0.7152 * linear(rgb[1]) + 0.0722 * linear(rgb[2])
}

fn contrast_ratio(foreground: [u8; 3], background: [u8; 3]) -> f64 {
    let foreground = relative_luminance(foreground);
    let background = relative_luminance(background);
    (foreground.max(background) + 0.05) / (foreground.min(background) + 0.05)
}

#[test]
fn stylesheet_uses_at_most_seven_canonical_type_steps() {
    let allowed = [
        "0.68em", "0.74em", "0.80em", "0.86em", "0.92em", "1.00em", "1.22em",
    ];
    let sizes = validate_declarations(CSS, "font-size", &allowed).unwrap();

    assert!(!sizes.is_empty());
    assert!(sizes.len() <= allowed.len());
}

#[test]
fn stylesheet_uses_standard_font_weights() {
    let allowed = ["400", "500", "600", "700"];
    let weights = validate_declarations(CSS, "font-weight", &allowed).unwrap();

    assert!(!weights.is_empty());
}

#[test]
fn settings_keep_clear_page_and_preference_group_hierarchy() {
    let page_title = block(".settings-page-title {");
    assert!(page_title.contains("font-size: 1.22em;"));

    let group = block(".settings-list {");
    assert!(group.contains("background: @ft_bg_1;"));
    assert!(group.contains("border: 1px solid @ft_line;"));
    assert!(group.contains("border-radius: 8px;"));

    let nav_heading = block(".settings-nav-heading {");
    assert!(!nav_heading.contains("text-transform: uppercase;"));

    let info_hover = block(".settings-list row.settings-info-row:hover {");
    assert!(info_hover.contains("background: transparent;"));
}

#[test]
fn stylesheet_keeps_hex_colors_in_named_tokens() {
    let css = without_comments(CSS);
    for (line_number, line) in css.lines().enumerate() {
        if line.contains('#') {
            assert!(
                line.trim_start().starts_with("@define-color "),
                "raw hex color outside token definitions on line {}: {line}",
                line_number + 1
            );
        }
    }
}

#[test]
fn running_state_always_uses_the_success_token() {
    for selector in [
        ".workspace-status-badge.running,",
        ".pane-agent-badge.running {",
        ".agent-lifecycle.running {",
    ] {
        assert!(
            block(selector).contains("color: @ft_success;"),
            "{selector} must use @ft_success"
        );
    }
}

#[test]
fn faint_text_meets_wcag_aa_on_every_design_surface() {
    let text = defined_hex_color("ft_text_4");
    for surface in [
        "ft_bg_0",
        "ft_bg_1",
        "ft_bg_2",
        "ft_bg_3",
        "ft_bg_recessed",
        "ft_bg_workbench",
        "ft_bg_stage",
        "ft_bg_control",
        "ft_bg_selected",
    ] {
        let ratio = contrast_ratio(text, defined_hex_color(surface));
        assert!(
            ratio >= 4.5,
            "@ft_text_4 contrast on @{surface} is {ratio:.2}:1"
        );
    }
}

#[test]
fn status_text_is_not_rendered_as_a_pill() {
    let header_title = block(".app-header-title {");
    assert!(header_title.contains("border-radius: 6px;"));
    assert!(!header_title.contains("border-radius: 999px;"));

    let kind = block(".notification-kind {");
    assert!(!kind.contains("border-radius:"));
    assert!(!kind.contains("background"));
    for selector in [
        ".notification-kind.info {",
        ".notification-kind.error {",
        ".notification-kind.prompt {",
        ".notification-kind.custom {",
    ] {
        let kind = block(selector);
        assert!(!kind.contains("background"));
        assert!(!kind.contains("border-radius:"));
    }
}

#[test]
fn stylesheet_uses_the_documented_radius_scale() {
    let radii =
        validate_declarations(CSS, "border-radius", &["0", "4px", "6px", "8px", "999px"]).unwrap();

    assert!(!radii.is_empty());
}

#[test]
fn declaration_validation_is_fail_closed() {
    let type_scale = [
        "0.68em", "0.74em", "0.80em", "0.86em", "0.92em", "1.00em", "1.22em",
    ];
    assert!(validate_declarations(
        "label {\n  font-size: 0.681em;\n}",
        "font-size",
        &type_scale,
    )
    .is_err());
    assert!(validate_declarations(
        "label {\n  font-weight:550;\n}",
        "font-weight",
        &["400", "500", "600", "700"],
    )
    .is_err());
    assert!(validate_declarations(
        "label {\n  border-radius:5px;\n}",
        "border-radius",
        &["0", "4px", "6px", "8px", "999px"],
    )
    .is_err());
    assert!(
        validate_declarations("label {\n  font-size: 0.80em\n}", "font-size", &type_scale,)
            .is_err()
    );
    assert!(
        validate_declarations("label {\n  font-size 0.80em;\n}", "font-size", &type_scale,)
            .is_err()
    );
}
