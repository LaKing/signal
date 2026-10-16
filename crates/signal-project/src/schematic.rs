//! Schematic positions (ADR 0026 §3, `docs/project-format.md` §11): the
//! optional `ui/schematic.toml`, presentation only, one table per graph
//! level: `[top]` for the project's top level and `[composite.<name>]` for
//! each composite, each mapping an instance to `{ x = …, y = … }`. Edited
//! with `toml_edit`, so comments and everything else in the file are kept.
//! Control-thread code: it works on text, the caller reads and writes files.

use std::collections::BTreeMap;

use toml_edit::{DocumentMut, InlineTable, Item, Table, Value};

/// The file's path in a project folder.
pub const SCHEMATIC_FILE: &str = "ui/schematic.toml";

/// Stored positions: by level (`""` for the top level, else the composite's
/// name), by instance.
pub type Positions = BTreeMap<String, BTreeMap<String, (i64, i64)>>;

fn parse(text: &str) -> Result<DocumentMut, String> {
    text.parse::<DocumentMut>()
        .map_err(|e| format!("{SCHEMATIC_FILE} does not parse: {}", e.message()))
}

/// The positions in `text`. Entries that are not `{ x = integer, y =
/// integer }` tables, and tables other than the levels', are ignored: the file
/// is presentation only, so nothing in it can stop a project.
pub fn read_positions(text: &str) -> Result<Positions, String> {
    let document = parse(text)?;
    let mut positions = Positions::new();
    let mut take = |level: &str, table: &dyn toml_edit::TableLike| {
        let entries = positions.entry(level.to_owned()).or_default();
        for (instance, item) in table.iter() {
            let Some(entry) = item.as_table_like() else {
                continue;
            };
            let coordinate = |key| entry.get(key).and_then(Item::as_integer);
            if let (Some(x), Some(y)) = (coordinate("x"), coordinate("y")) {
                entries.insert(instance.to_owned(), (x, y));
            }
        }
    };
    if let Some(top) = document.get("top").and_then(Item::as_table_like) {
        take("", top);
    }
    if let Some(composites) = document.get("composite").and_then(Item::as_table_like) {
        for (name, item) in composites.iter() {
            if let Some(table) = item.as_table_like() {
                take(name, table);
            }
        }
    }
    positions.retain(|_, entries| !entries.is_empty());
    Ok(positions)
}

/// `text` with `instance` of `level` (`""`: the top level) at `(x, y)`.
pub fn set_position(
    text: &str,
    level: &str,
    instance: &str,
    x: i64,
    y: i64,
) -> Result<String, String> {
    set_positions(text, level, &[(instance, x, y)])
}

/// `text` with each `(instance, x, y)` of `moves` of `level` (`""`: the top
/// level) at its position, in order: parsed once and written once, however
/// many (a group of boxes dragged together).
pub fn set_positions(
    text: &str,
    level: &str,
    moves: &[(&str, i64, i64)],
) -> Result<String, String> {
    let mut document = parse(text)?;
    let table = if level.is_empty() {
        document
            .entry("top")
            .or_insert(Item::Table(Table::new()))
            .as_table_mut()
    } else {
        let mut composites = Table::new();
        composites.set_implicit(true);
        document
            .entry("composite")
            .or_insert(Item::Table(composites))
            .as_table_mut()
            .and_then(|c| {
                c.entry(level)
                    .or_insert(Item::Table(Table::new()))
                    .as_table_mut()
            })
    }
    .ok_or_else(|| format!("{SCHEMATIC_FILE}: the table of level `{level}` is not a table"))?;
    for &(instance, x, y) in moves {
        let mut position = InlineTable::new();
        position.insert("x", Value::from(x));
        position.insert("y", Value::from(y));
        match table.get_mut(instance) {
            Some(Item::Value(old)) => {
                let decor = old.decor().clone();
                *old = Value::InlineTable(position);
                *old.decor_mut() = decor;
            }
            _ => {
                table.insert(instance, Item::Value(Value::InlineTable(position)));
            }
        }
    }
    Ok(document.to_string())
}

/// `text` without the table of `level` (`""`: the top level).
pub fn remove_level(text: &str, level: &str) -> Result<String, String> {
    let mut document = parse(text)?;
    if level.is_empty() {
        document.remove("top");
    } else if let Some(composites) = document
        .get_mut("composite")
        .and_then(Item::as_table_like_mut)
    {
        composites.remove(level);
        if composites.is_empty() {
            document.remove("composite");
        }
    }
    Ok(document.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_positions_are_written_at_once_as_one_by_one() {
        let text = "# by hand\n[top]\ndrums = { x = 1, y = 2 } # kept\n";
        let at_once = set_positions(
            text,
            "",
            &[("drums", 7, 8), ("out", 200, 0), ("lfo", -3, 4)],
        )
        .unwrap();
        let mut one_by_one = text.to_owned();
        for (instance, x, y) in [("drums", 7, 8), ("out", 200, 0), ("lfo", -3, 4)] {
            one_by_one = set_position(&one_by_one, "", instance, x, y).unwrap();
        }
        assert_eq!(at_once, one_by_one);
        assert_eq!(
            at_once,
            "# by hand\n[top]\ndrums = { x = 7, y = 8 } # kept\nout = { x = 200, y = 0 }\nlfo = { x = -3, y = 4 }\n"
        );
        let inner = set_positions("", "kick", &[("osc", 0, 30), ("env", 5, 6)]).unwrap();
        assert_eq!(
            inner,
            "[composite.kick]\nosc = { x = 0, y = 30 }\nenv = { x = 5, y = 6 }\n"
        );
        assert_eq!(
            set_positions(text, "", &[]).unwrap(),
            text,
            "nothing to write changes nothing"
        );
        assert!(set_positions("top = 3\n", "", &[("a", 0, 0)]).is_err());
    }

    #[test]
    fn positions_are_written_one_table_per_level_and_read_back() {
        let text = set_position("", "", "drums", 10, -4).unwrap();
        let text = set_position(&text, "kick", "osc", 0, 30).unwrap();
        let text = set_position(&text, "", "out", 200, 0).unwrap();
        assert_eq!(
            text,
            "[top]\ndrums = { x = 10, y = -4 }\nout = { x = 200, y = 0 }\n\n[composite.kick]\nosc = { x = 0, y = 30 }\n"
        );
        let positions = read_positions(&text).unwrap();
        assert_eq!(positions[""]["drums"], (10, -4));
        assert_eq!(positions["kick"]["osc"], (0, 30));
    }

    #[test]
    fn an_edit_keeps_comments_and_other_entries() {
        let text = "# hand-made\n[top]\n# the drums\ndrums = { x = 1, y = 2 } # here\nout = { x = 5, y = 5 }\n";
        let edited = set_position(text, "", "drums", 7, 8).unwrap();
        assert_eq!(
            edited,
            "# hand-made\n[top]\n# the drums\ndrums = { x = 7, y = 8 } # here\nout = { x = 5, y = 5 }\n"
        );
    }

    #[test]
    fn a_level_is_removed_alone() {
        let text = set_position("", "", "drums", 1, 2).unwrap();
        let text = set_position(&text, "kick", "osc", 3, 4).unwrap();
        let without_kick = remove_level(&text, "kick").unwrap();
        assert_eq!(without_kick, "[top]\ndrums = { x = 1, y = 2 }\n");
        let without_top = remove_level(&text, "").unwrap();
        assert_eq!(
            read_positions(&without_top)
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            ["kick"]
        );
    }

    #[test]
    fn odd_entries_are_ignored_and_bad_syntax_is_an_error() {
        let positions = read_positions(
            "[top]\na = { x = 1 }\nb = 3\nc = { x = 1, y = 2 }\n[other]\nd = { x = 0, y = 0 }\n",
        )
        .unwrap();
        assert_eq!(positions.len(), 1);
        assert_eq!(positions[""].len(), 1);
        assert_eq!(positions[""]["c"], (1, 2));
        assert!(read_positions("[top\n").is_err());
        assert!(set_position("top = 3\n", "", "a", 0, 0).is_err());
    }
}
