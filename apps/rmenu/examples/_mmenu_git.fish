# Shared implementation for the sourceable pickers. No GUI runs on source.

# Prints the mmenu executable: MMENU_BIN, then PATH, then this checkout's bundle.
function _mmenu_bin
    set -l menu
    if set -q MMENU_BIN; and test -n "$MMENU_BIN"
        set menu "$MMENU_BIN"
    else if command -q mmenu
        set menu (command -s mmenu)
    else
        set -l here (path dirname -- (status filename))
        set menu "$here/../build/mmenu.app/Contents/MacOS/mmenu"
    end
    if not test -x "$menu"
        printf 'mmenu: build the bundle first, or set MMENU_BIN to its executable path\n' >&2
        return 127
    end
    printf '%s\n' "$menu"
end

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

# Row producers print JSONL. They run before a standalone menu, or inside ghqm's
# menu as pushed sources, where the parent's choice arrives as MMENU_VALUE.
function _mmenu_repo_rows
    set -l roots (command ghq root --all)
    or return
    set -l paths (command ghq list -p)
    or return
    # The value is the full path, so accepting needs no second ghq lookup. The ID is
    # the root-relative name ghq uses.
    printf '%s\n' $paths | command jq -Rc --args '
        select(length > 0) | . as $path |
        ([$ARGS.positional[] | select(. as $root | $path | startswith($root + "/"))] | max_by(length)) as $root |
        (if $root then $path[($root | length) + 1:] else $path end) as $name |
        {id: $name, title: $name, value: $path}
    ' $roots
end

function _mmenu_worktree_rows --argument-names repo
    set -l inventory (command git -C "$repo" wt list --json)
    or return
    printf '%s\n' $inventory | command jq -c '
        if type != "array" then error("Expected git wt JSON array") else .[] end |
        select(.bare | not) |
        {id: .path, title: (.branch // "detached"), subtitle: .path,
         detail: (if .current then "Current" else "" end), value: .path}
    '
end

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

# The Rex actions every picker level offers, as mmenu bindings.
function _mmenu_rex_bindings
    if test "$TERM_PROGRAM" = rex; and command -q rex
        printf '%s' '[{"key":"ctrl+v","id":"split-right","label":"Split right","action":"accept"},{"key":"ctrl+s","id":"split-below","label":"Split below","action":"accept"},{"key":"ctrl+t","id":"project-tab","label":"Project tab","action":"accept"}]'
    else
        printf '[]'
    end
end

function _mmenu_git_pick --argument-names prompt query
    set -l menu (_mmenu_bin)
    or return
    set -l config (command mktemp -t mmenu-git.XXXXXX)
    or return
    command jq -n --arg prompt "$prompt" --argjson rex (_mmenu_rex_bindings) '
        {
            prompt: $prompt,
            theme: "rex",
            bindings: $rex
        }
    ' >"$config"
    set -l code $status
    if test $code -eq 0
        # Candidate JSONL arrives on stdin.
        command "$menu" --config "$config" --input jsonl --json --width 720 --query "$query"
        set code $status
    end
    command rm -f -- "$config"
    return $code
end

# Shared by gwtm and gmemom: parse -C/--directory and the query words, list rows for
# the repository with ROWS, pick one and dispatch it. A nonempty KEEP_REPO passes the
# repository to Rex as the project label (memos are not repositories themselves).
function _mmenu_git_standalone --argument-names name prompt rows empty keep_repo
    argparse --name $name 'C/directory=' -- $argv[6..]; or return
    _mmenu_require $name git jq; or return
    set -l repo "$PWD"
    set -q _flag_directory; and set repo "$_flag_directory"
    test -d "$repo"; or return 1
    # Absolute even when -C was relative: Rex gets the repository as a label.
    set repo (path resolve -- "$repo" | string collect)
    set -l items ($rows "$repo")
    or return
    if not set -q items[1]
        printf '%s: %s\n' $name $empty >&2
        return 1
    end
    set -l result (printf '%s\n' $items | _mmenu_git_pick $prompt (string join ' ' -- $argv))
    or return
    set -l label
    test -n "$keep_repo"; and set label "$repo"
    _mmenu_git_dispatch "$result" "$label"
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
    if test "$TERM_PROGRAM" != rex; or not command -q rex
        return 1
    end
    # Memos keep their source repository's project label, not the memo slug.
    if test -n "$source_repo"
        _git_dir_action $target "$directory" "$source_repo"
    else
        _git_dir_action $target "$directory"
    end
end
