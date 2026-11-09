# Fails with status 127 unless every named command is on PATH.
function _mmenu_require --argument-names name
    for dependency in $argv[2..]
        if not command -q $dependency
            printf '%s: missing command: %s\n' $name $dependency >&2
            return 127
        end
    end
end

# Prints the one chosen value under PATH (a JSON array, e.g. ["path",0]) of an mmenu
# --json result, NUL-terminated so spaces and newlines survive. Fails unless that level
# chose exactly one item whose value is a nonempty string without NUL.
function _mmenu_single_value --argument-names result path
    printf '%s\n' "$result" | command jq -j --argjson at "$path" '
        getpath($at) |
        if (.items | length == 1) and (.items[0].value | type == "string" and length > 0 and (contains("\u0000") | not))
        then .items[0].value, "\u0000" else error("expected one chosen value") end
    '
end

# Memo rows as JSONL, for ghqr's memo level, where the chosen repository arrives as MMENU_VALUE.
function _mmenu_memo_rows --argument-names repo
    set -l scope (command git -C "$repo" memo path)
    or return
    if test (count $scope) -ne 1; or not test -d "$scope[1]"
        printf 'mmenu: no existing memo scope\n' >&2
        return 1
    end
    set -l inventory (command git -C "$repo" memo list)
    or return
    printf '%s\n' $inventory | command jq -Rc --arg scope "$scope" '
        select(length > 0) | split("\t") |
        if .[0] == "." or .[0] == ".." or .[0] == "" or (.[0] | contains("/"))
        then error("Expected an immediate memo directory name") else . end |
        {id: .[0], title: .[0], subtitle: (.[1:] | join("\t")), value: ($scope + "/" + .[0])}
    '
end

# Succeeds when this shell runs in a Rex block and the rex command can act on it.
function _mmenu_in_rex
    test "$TERM_PROGRAM" = rex; and set -q REX_SESSION REX_BLOCK; and command -q rex
end

# The Rex actions every picker level offers, as mmenu bindings.
function _mmenu_rex_bindings
    if _mmenu_in_rex
        printf '%s' '[{"key":"ctrl+v","id":"split-right","label":"Split right","action":"accept"},{"key":"ctrl+s","id":"split-below","label":"Split below","action":"accept"},{"key":"ctrl+t","id":"project-tab","label":"Project tab","action":"accept"}]'
    else
        printf '[]'
    end
end

function _mmenu_git_dispatch --argument-names result source_repo
    # Anything but the exact action names below is refused by the switch.
    set -l action (printf '%s\n' "$result" | command jq -er .action)
    or return 1
    # NUL framing preserves spaces, tabs and embedded newlines in worktree paths.
    set -l directory (_mmenu_single_value "$result" '[]' | string split0)
    or return 1
    if not test -d "$directory"
        printf 'mmenu: selected directory no longer exists\n' >&2
        return 1
    end
    set -l target
    switch "$action"
        case accept
            cd -- "$directory"
            return
        case split-right
            set target right
        case split-below
            set target below
        case project-tab
            set target tab
        case '*'
            printf 'mmenu: unexpected action: %s\n' "$action" >&2
            return 1
    end
    _mmenu_in_rex; or return 1
    # Memos keep their source repository's project label, not the memo slug.
    if test -n "$source_repo"
        _git_dir_action $target "$directory" "$source_repo"
    else
        _git_dir_action $target "$directory"
    end
end
