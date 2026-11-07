use crate::fuzzy::fuzzy_match;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandId {
    Add,
    Find,
    List,
    Review,
    Edit,
    Delete,
    Restore,
    Stats,
    Export,
    Import,
    Backup,
    Sync,
    Theme,
    Ime,
    Help,
    Quit,
}

#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub id: CommandId,
    pub name: &'static str,
    pub usage: &'static str,
    pub desc: &'static str,
    pub aliases: &'static [&'static str],
    pub requires_arg: bool,
}

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        id: CommandId::Add,
        name: "add",
        usage: "/add [word]",
        desc: "create a new word",
        aliases: &["new"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Find,
        name: "find",
        usage: "/find [query]",
        desc: "search words",
        aliases: &["search"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::List,
        name: "list",
        usage: "/list",
        desc: "browse all words",
        aliases: &["words"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Review,
        name: "review",
        usage: "/review <word>",
        desc: "practise a word now",
        aliases: &["study"],
        requires_arg: true,
    },
    CommandSpec {
        id: CommandId::Edit,
        name: "edit",
        usage: "/edit <word>",
        desc: "edit meaning or examples",
        aliases: &[],
        requires_arg: true,
    },
    CommandSpec {
        id: CommandId::Delete,
        name: "delete",
        usage: "/delete <word>",
        desc: "archive a word",
        aliases: &["archive"],
        requires_arg: true,
    },
    CommandSpec {
        id: CommandId::Restore,
        name: "restore",
        usage: "/restore <word>",
        desc: "restore an archived word",
        aliases: &[],
        requires_arg: true,
    },
    CommandSpec {
        id: CommandId::Stats,
        name: "stats",
        usage: "/stats",
        desc: "progress and weak words",
        aliases: &["progress"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Export,
        name: "export",
        usage: "/export [path]",
        desc: "write a JSON backup",
        aliases: &[],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Import,
        name: "import",
        usage: "/import <path>",
        desc: "merge a JSON backup",
        aliases: &[],
        requires_arg: true,
    },
    CommandSpec {
        id: CommandId::Backup,
        name: "backup",
        usage: "/backup",
        desc: "write a SQLite snapshot",
        aliases: &[],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Sync,
        name: "sync",
        usage: "/sync [dir]",
        desc: "merge and publish JSON snapshots",
        aliases: &["pull", "push"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Theme,
        name: "theme",
        usage: "/theme [auto|dark|light|mono]",
        desc: "change the colour theme",
        aliases: &[],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Ime,
        name: "ime",
        usage: "/ime [on|off|en|zh]",
        desc: "Chinese/English input switching",
        aliases: &["input"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Help,
        name: "help",
        usage: "/help",
        desc: "keys, commands and paths",
        aliases: &["keys"],
        requires_arg: false,
    },
    CommandSpec {
        id: CommandId::Quit,
        name: "quit",
        usage: "/quit",
        desc: "leave eword",
        aliases: &["exit", "q"],
        requires_arg: false,
    },
];

pub fn find_spec(name: &str) -> Option<&'static CommandSpec> {
    let name = name.trim_start_matches('/').to_ascii_lowercase();
    COMMANDS
        .iter()
        .find(|spec| spec.name == name || spec.aliases.iter().any(|alias| *alias == name))
}

pub fn filtered(query: &str) -> Vec<&'static CommandSpec> {
    let needle = query
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim();
    if needle.is_empty() {
        return COMMANDS.iter().collect();
    }

    let mut scored: Vec<(i32, usize, &'static CommandSpec)> = COMMANDS
        .iter()
        .enumerate()
        .filter_map(|(order, spec)| {
            let mut best = fuzzy_match(needle, spec.name);
            for alias in spec.aliases {
                if let Some(candidate) = fuzzy_match(needle, alias)
                    && best
                        .as_ref()
                        .is_none_or(|current| candidate.score > current.score)
                {
                    best = Some(candidate);
                }
            }
            best.map(|matched| (matched.score, order, spec))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, _, spec)| spec).collect()
}

pub fn parse(input: &str) -> Option<(&'static CommandSpec, String)> {
    let rest = input.strip_prefix('/')?;
    let mut parts = rest.splitn(2, char::is_whitespace);
    let name = parts.next().unwrap_or("").trim();
    if name.is_empty() {
        return None;
    }
    let spec = find_spec(name)?;
    let args = parts.next().unwrap_or("").trim().to_string();
    Some((spec, args))
}

pub fn suggestions(input: &str) -> Vec<&'static CommandSpec> {
    filtered(input).into_iter().take(3).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_and_rest_as_single_argument() {
        let (spec, args) = parse("/review take it easy").unwrap();
        assert_eq!(spec.id, CommandId::Review);
        assert_eq!(args, "take it easy");
    }

    #[test]
    fn resolves_aliases() {
        assert_eq!(find_spec("exit").unwrap().id, CommandId::Quit);
        assert_eq!(find_spec("search").unwrap().id, CommandId::Find);
    }

    #[test]
    fn filters_by_prefix() {
        let hits = filtered("/fin");
        assert_eq!(hits[0].name, "find");
    }

    #[test]
    fn rejects_bare_slash() {
        assert!(parse("/").is_none());
    }
}
