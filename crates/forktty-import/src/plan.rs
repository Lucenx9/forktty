//! Source→destination import-plan resolver, ported from cmux's
//! `BrowserImportPlanResolver`. Pure: takes selected source profiles + existing
//! destination profiles, returns an `ImportPlan`. No IO.

use forktty_core::{ProfileId, ProfileMeta};

use crate::model::{ImportDestination, ImportEntry, ImportMode, ImportPlan, SourceProfile};

fn normalized<'a>(name: &'a str) -> std::borrow::Cow<'a, str> {
    let trimmed = name.trim();
    if trimmed.chars().any(|c| c.is_uppercase()) {
        std::borrow::Cow::Owned(trimmed.to_lowercase())
    } else {
        std::borrow::Cow::Borrowed(trimmed)
    }
}

/// First destination whose display name matches `source_name` (trimmed, case-insensitive).
fn matching_destination(source_name: &str, destinations: &[ProfileMeta]) -> Option<ProfileId> {
    let norm = normalized(source_name);
    if norm.is_empty() {
        return None;
    }
    destinations
        .iter()
        .find(|d| normalized(&d.display_name) == norm)
        .map(|d| d.id)
}

/// A create-name not already taken (normalized): `base`, then `base (2)`, `base (3)`, …
/// Empty base falls back to `Profile`.
fn next_create_name(base: &str, taken: &std::collections::HashSet<String>) -> String {
    let trimmed = base.trim();
    let resolved = if trimmed.is_empty() {
        "Profile"
    } else {
        trimmed
    };
    if !taken.contains(normalized(resolved).as_ref()) {
        return resolved.to_string();
    }
    let mut suffix = 2;
    loop {
        let candidate = format!("{resolved} ({suffix})");
        if !taken.contains(normalized(&candidate).as_ref()) {
            return candidate;
        }
        suffix += 1;
    }
}

/// cmux `defaultPlan`: ≤1 source → SingleDestination (match-by-name else preferred);
/// >1 source → SeparateProfiles.
pub fn resolve_default_plan(
    selected: &[SourceProfile],
    destinations: &[ProfileMeta],
    preferred_single_destination: ProfileId,
) -> ImportPlan {
    if selected.len() <= 1 {
        let destination = selected
            .first()
            .and_then(|s| matching_destination(&s.display_name, destinations))
            .map(ImportDestination::Existing)
            .unwrap_or(ImportDestination::Existing(preferred_single_destination));
        return ImportPlan {
            mode: ImportMode::SingleDestination,
            entries: selected
                .iter()
                .map(|s| ImportEntry {
                    sources: vec![s.clone()],
                    destination: destination.clone(),
                })
                .collect(),
        };
    }
    resolve_separate_profiles_plan(selected, destinations)
}

/// cmux `separateProfilesPlan`: one destination per source; reuse a same-named
/// existing destination, else create a stable de-duplicated name.
pub fn resolve_separate_profiles_plan(
    selected: &[SourceProfile],
    destinations: &[ProfileMeta],
) -> ImportPlan {
    let mut reserved: std::collections::HashSet<String> = destinations
        .iter()
        .map(|d| normalized(&d.display_name).into_owned())
        .collect();
    let entries = selected
        .iter()
        .map(|s| {
            let destination = if let Some(id) = matching_destination(&s.display_name, destinations)
            {
                ImportDestination::Existing(id)
            } else {
                let name = next_create_name(&s.display_name, &reserved);
                reserved.insert(normalized(&name).into_owned());
                ImportDestination::Create(name)
            };
            ImportEntry {
                sources: vec![s.clone()],
                destination,
            }
        })
        .collect();
    ImportPlan {
        mode: ImportMode::SeparateProfiles,
        entries,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;
    use forktty_core::{ProfileId, ProfileMeta};

    fn src(name: &str, default: bool) -> SourceProfile {
        SourceProfile {
            family: BrowserFamily::Firefox,
            display_name: name.to_string(),
            path: format!("/tmp/src-{}", name.trim()),
            is_default: default,
        }
    }

    fn dest(name: &str, id: ProfileId) -> ProfileMeta {
        ProfileMeta {
            id,
            display_name: name.to_string(),
            created_at: 0,
            is_default: false,
        }
    }

    #[test]
    fn default_plan_uses_separate_mode_for_multiple_sources() {
        let default_id = ProfileId::default();
        let plan = resolve_default_plan(
            &[src("You", true), src("austin", false)],
            &[dest("Default", default_id)],
            default_id,
        );
        assert_eq!(plan.mode, ImportMode::SeparateProfiles);
        assert_eq!(plan.entries.len(), 2);
        let names: Vec<_> = plan
            .entries
            .iter()
            .map(|e| e.sources[0].display_name.clone())
            .collect();
        assert_eq!(names, vec!["You", "austin"]);
    }

    #[test]
    fn default_plan_uses_single_destination_for_one_source() {
        let default_id = ProfileId::default();
        let plan = resolve_default_plan(&[src("You", true)], &[], default_id);
        assert_eq!(plan.mode, ImportMode::SingleDestination);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].sources[0].display_name, "You");
        assert_eq!(
            plan.entries[0].destination,
            ImportDestination::Existing(default_id)
        );
    }

    #[test]
    fn separate_plan_reuses_existing_same_named_destination() {
        let work_id = ProfileId::new();
        let plan = resolve_separate_profiles_plan(&[src(" you ", true)], &[dest("You", work_id)]);
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(
            plan.entries[0].destination,
            ImportDestination::Existing(work_id)
        );
    }

    #[test]
    fn separate_plan_stable_create_names_on_collision() {
        let plan = resolve_separate_profiles_plan(&[src("Work", true), src("Work", false)], &[]);
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(
            plan.entries[0].destination,
            ImportDestination::Create("Work".to_string())
        );
        assert_eq!(
            plan.entries[1].destination,
            ImportDestination::Create("Work (2)".to_string())
        );
    }

    #[test]
    fn empty_selection_yields_empty_single_destination_plan() {
        let id = ProfileId::default();
        let plan = resolve_default_plan(&[], &[], id);
        assert_eq!(plan.mode, ImportMode::SingleDestination);
        assert!(plan.entries.is_empty());
    }
}
