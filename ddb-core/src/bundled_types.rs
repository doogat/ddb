//! Bundled _typedef doogat templates for common types.
//! These can be installed via `ddb type install <name>`.

const PROJECT_TYPEDEF: &str = "\
---
title: project
type: _typedef
columns:
  - name: completed
    data_type: BOOLEAN
    zone: frontmatter
  - name: deliverable
    data_type: TEXT
    zone: frontmatter
  - name: parent
    data_type: TEXT
    zone: reference
    references: project
  - name: ticket
    data_type: TEXT
    zone: reference
  - name: us
    data_type: TEXT
    zone: reference
crdt_strategy: preset:append-log
template_sections:
  - Description
  - Log
  - Plan
  - Solution
---
";

const CONTACT_TYPEDEF: &str = "\
---
title: contact
type: _typedef
columns:
  - name: aliases
    data_type: TEXT
    zone: frontmatter
  - name: contact-type
    data_type: TEXT
    zone: frontmatter
  - name: email
    data_type: TEXT
    zone: frontmatter
    search_boost: 1.5
crdt_strategy: preset:default
folder: true
template_sections:
  - First contact
  - Timeline
  - Relationships
---
";

const LITERATURE_NOTE_TYPEDEF: &str = "\
---
title: literature-note
type: _typedef
columns:
  - name: author
    data_type: TEXT
    zone: frontmatter
  - name: source
    data_type: TEXT
    zone: frontmatter
  - name: year
    data_type: INTEGER
    zone: frontmatter
  - name: url
    data_type: TEXT
    zone: frontmatter
crdt_strategy: preset:default
template_sections:
  - Summary
  - Key Arguments
  - Quotes
  - Personal Response
---
";

const MEETING_MINUTES_TYPEDEF: &str = "\
---
title: meeting-minutes
type: _typedef
columns:
  - name: date
    data_type: TEXT
    zone: frontmatter
  - name: attendees
    data_type: TEXT
    zone: frontmatter
  - name: location
    data_type: TEXT
    zone: frontmatter
crdt_strategy: preset:append-log
template_sections:
  - Agenda
  - Log
  - Decisions
  - Action Items
---
";

const KANBAN_TYPEDEF: &str = "\
---
title: kanban
type: _typedef
columns:
  - name: status
    data_type: TEXT
    zone: frontmatter
    allowed_values:
      - backlog
      - todo
      - doing
      - done
      - blocked
    default_value: backlog
  - name: priority
    data_type: TEXT
    zone: frontmatter
    allowed_values:
      - low
      - medium
      - high
      - critical
  - name: assignee
    data_type: TEXT
    zone: frontmatter
  - name: due
    data_type: TEXT
    zone: frontmatter
  - name: parent
    data_type: TEXT
    zone: reference
    references: kanban
crdt_strategy: preset:last-writer-wins
template_sections:
  - Description
  - Acceptance Criteria
---
";

/// Get bundled type definition content by name.
pub fn get_bundled_type(name: &str) -> Option<&'static str> {
    match name {
        "project" => Some(PROJECT_TYPEDEF),
        "contact" => Some(CONTACT_TYPEDEF),
        "literature-note" => Some(LITERATURE_NOTE_TYPEDEF),
        "meeting-minutes" => Some(MEETING_MINUTES_TYPEDEF),
        "kanban" => Some(KANBAN_TYPEDEF),
        _ => None,
    }
}

/// List all available bundled type names.
pub fn list_bundled_types() -> &'static [&'static str] {
    &[
        "contact",
        "kanban",
        "literature-note",
        "meeting-minutes",
        "project",
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_project_bundled_type() {
        let content = get_bundled_type("project");
        assert!(content.is_some());
        let content = content.unwrap();
        assert!(content.contains("title: project"));
        assert!(content.contains("type: _typedef"));
        assert!(content.contains("completed"));
        assert!(content.contains("preset:append-log"));
    }

    #[test]
    fn get_contact_bundled_type() {
        let content = get_bundled_type("contact");
        assert!(content.is_some());
        let content = content.unwrap();
        assert!(content.contains("title: contact"));
        assert!(content.contains("email"));
        assert!(content.contains("search_boost: 1.5"));
    }

    #[test]
    fn get_unknown_bundled_type() {
        assert!(get_bundled_type("unknown").is_none());
    }

    #[test]
    fn list_bundled_types_returns_all() {
        let types = list_bundled_types();
        assert_eq!(types.len(), 5);
        assert!(types.contains(&"project"));
        assert!(types.contains(&"contact"));
        assert!(types.contains(&"literature-note"));
        assert!(types.contains(&"meeting-minutes"));
        assert!(types.contains(&"kanban"));
    }

    #[test]
    fn get_literature_note_bundled_type() {
        let content = get_bundled_type("literature-note").unwrap();
        assert!(content.contains("title: literature-note"));
        assert!(content.contains("author"));
        assert!(content.contains("source"));
        assert!(content.contains("year"));
        assert!(content.contains("Summary"));
    }

    #[test]
    fn get_meeting_minutes_bundled_type() {
        let content = get_bundled_type("meeting-minutes").unwrap();
        assert!(content.contains("title: meeting-minutes"));
        assert!(content.contains("attendees"));
        assert!(content.contains("preset:append-log"));
        assert!(content.contains("Decisions"));
    }

    #[test]
    fn get_kanban_bundled_type() {
        let content = get_bundled_type("kanban").unwrap();
        assert!(content.contains("title: kanban"));
        assert!(content.contains("allowed_values"));
        assert!(content.contains("default_value: backlog"));
        assert!(content.contains("preset:last-writer-wins"));
        assert!(content.contains("references: kanban"));
        assert!(content.contains("Acceptance Criteria"));
    }
}
