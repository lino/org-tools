// Copyright (C) 2026 org-tools contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Shell completion script rendering (`bash`, `zsh`, `fish`) with dynamic `org complete` hooks.

use std::io::Write as _;

use clap::Command;
use clap_complete::Shell;

/// Bash dynamic completion wrapper appended to the `clap_complete` base script.
const BASH_DYNAMIC_ADAPTER: &str = r#"
_org_dynamic_kind() {
    local prev="${COMP_WORDS[COMP_CWORD-1]}"
    case "$prev" in
        --to|--locator|--parent) echo "locator"; return ;;
        --blocker|--trigger) echo "id"; return ;;
        --context|-c) echo "context"; return ;;
        --query) echo "query"; return ;;
        --rule) echo "rule"; return ;;
    esac
    local cmd1="${COMP_WORDS[1]:-}"
    local cmd2="${COMP_WORDS[2]:-}"
    case "$cmd1:$cmd2" in
        show:*|refile:*|clock:in) echo "locator" ;;
        roam:node|roam:backlinks|roam:links) echo "roam-node" ;;
        query:search) echo "query" ;;
        query:next) echo "context" ;;
        update:set-state)
            if [[ $COMP_CWORD -eq 3 ]]; then echo "todo-state"; else echo "locator"; fi ;;
        update:set-scheduled|update:set-deadline)
            if [[ $COMP_CWORD -eq 3 ]]; then echo "date-spec"; else echo "locator"; fi ;;
        update:set-property|update:delete-property)
            if [[ $COMP_CWORD -eq 4 ]]; then echo "property"; else echo "locator"; fi ;;
        update:set-tags)
            if [[ $COMP_CWORD -eq 4 ]]; then echo "tag"; else echo "locator"; fi ;;
        update:dep) echo "id" ;;
    esac
}

_org_dynamic() {
    local cur="${COMP_WORDS[COMP_CWORD]:-}"
    if [[ "$cur" != -* ]]; then
        local kind
        kind="$(_org_dynamic_kind)"
        if [[ -n "$kind" ]]; then
            local IFS=$'\n'
            COMPREPLY=( $(compgen -W "$(org complete "$kind" "$cur" --shell bash 2>/dev/null)" -- "$cur") )
            if [[ ${#COMPREPLY[@]} -gt 0 ]]; then
                return 0
            fi
        fi
    fi
    _org "$@"
}

complete -F _org_dynamic -o bashdefault -o default org
"#;

/// Zsh dynamic completion wrapper appended to the `clap_complete` base script.
const ZSH_DYNAMIC_ADAPTER: &str = r#"
_org_complete_kind() {
    local kind="$1"
    local -a items
    local line cand desc
    while IFS=$'\t' read -r cand desc; do
        [[ -z "$cand" ]] && continue
        if [[ -n "$desc" ]]; then
            items+=("${cand}:${desc}")
        else
            items+=("${cand}")
        fi
    done < <(org complete "$kind" "${PREFIX:-}" --shell zsh 2>/dev/null)
    if (( ${#items[@]} > 0 )); then
        _describe -t "$kind" "$kind" items && return 0
    fi
    return 1
}

_org_dynamic_kind() {
    local prev="${words[CURRENT-1]:-}"
    case "$prev" in
        --to|--locator|--parent) echo "locator"; return ;;
        --blocker|--trigger) echo "id"; return ;;
        --context|-c) echo "context"; return ;;
        --query) echo "query"; return ;;
        --rule) echo "rule"; return ;;
    esac
    local cmd1="${words[2]:-}"
    local cmd2="${words[3]:-}"
    case "$cmd1:$cmd2" in
        show:*|refile:*|clock:in) echo "locator" ;;
        roam:node|roam:backlinks|roam:links) echo "roam-node" ;;
        query:search) echo "query" ;;
        query:next) echo "context" ;;
        update:set-state)
            if (( CURRENT == 4 )); then echo "todo-state"; else echo "locator"; fi ;;
        update:set-scheduled|update:set-deadline)
            if (( CURRENT == 4 )); then echo "date-spec"; else echo "locator"; fi ;;
        update:set-property|update:delete-property)
            if (( CURRENT == 5 )); then echo "property"; else echo "locator"; fi ;;
        update:set-tags)
            if (( CURRENT == 5 )); then echo "tag"; else echo "locator"; fi ;;
        update:dep) echo "id" ;;
    esac
}

_org_dynamic_entry() {
    if [[ "${PREFIX:-}" != -* ]]; then
        local kind
        kind="$(_org_dynamic_kind)"
        if [[ -n "$kind" ]] && _org_complete_kind "$kind"; then
            return 0
        fi
    fi
    _org "$@"
}

if [[ "$funcstack[1]" = "_org" ]]; then
    _org_dynamic_entry "$@"
else
    compdef _org_dynamic_entry org
fi
"#;

/// Fish dynamic completion rules appended to the `clap_complete` base script.
const FISH_DYNAMIC_ADAPTER: &str = r#"
# Workspace-aware dynamic completions powered by `org complete`
complete -c org -n "__fish_seen_subcommand_from show" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from refile" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from refile" -l to -f -r -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from roam; and __fish_seen_subcommand_from node backlinks links" -f -a "(org complete roam-node (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from clock; and __fish_seen_subcommand_from in" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from clock" -l locator -f -r -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from query; and __fish_seen_subcommand_from search" -f -a "(org complete query (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from query; and __fish_seen_subcommand_from next" -l context -s c -f -r -a "(org complete context (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-state" -f -a "(org complete todo-state (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-state" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-scheduled set-deadline" -f -a "(org complete date-spec (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-scheduled set-deadline" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-property delete-property" -f -a "(org complete property (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-property delete-property" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-tags" -f -a "(org complete tag (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from set-tags" -f -a "(org complete locator (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from update; and __fish_seen_subcommand_from dep" -f -a "(org complete id (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from fmt" -l rule -f -r -a "(org complete rule (commandline -ct) --shell fish 2>/dev/null)"
complete -c org -n "__fish_seen_subcommand_from schema" -f -a "fmt query agenda columns show update clock-status clock-report jscalendar roam fleeting blocked stuck deps"
"#;

/// Renders the complete shell completion script (base `clap_complete` + dynamic adapter) for `shell`.
pub(crate) fn render_completion_script(shell: Shell, mut cmd: Command) -> String {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut cmd, "org", &mut buf);
    let mut script = String::from_utf8_lossy(&buf).into_owned();
    match shell {
        Shell::Bash => {
            script.push_str(BASH_DYNAMIC_ADAPTER);
        }
        Shell::Zsh => {
            if let Some(stripped) = script.trim_end().strip_suffix("\n_org \"$@\"") {
                script = format!("{stripped}\n{ZSH_DYNAMIC_ADAPTER}");
            } else {
                script.push_str(ZSH_DYNAMIC_ADAPTER);
            }
        }
        Shell::Fish => {
            script.push_str(FISH_DYNAMIC_ADAPTER);
        }
        _ => {}
    }
    script
}

/// Prints the rendered shell completion script for `shell` to `stdout`.
pub(crate) fn emit_completions(shell: Shell, cmd: Command) {
    let script = render_completion_script(shell, cmd);
    let _ = std::io::stdout().lock().write_all(script.as_bytes());
}
