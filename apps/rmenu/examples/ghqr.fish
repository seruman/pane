set -l examples (path resolve -- (path dirname -- (status filename)) | string collect)
source $examples/_mmenu_git.fish

function ghqr --inherit-variable examples --description 'Pick a ghq repository, worktree or memo in rmenu, in a Rex floating layer'
    _mmenu_require ghqr ghq git git-memo jq; or return
    # The split and tab keys call _git_dir_action, which lives in the user's fish config.
    if _mmenu_in_rex; and not functions -q _git_dir_action
        printf 'ghqr: missing function: _git_dir_action\n' >&2
        return 127
    end
    set -l menu $RMENU_BIN
    test -n "$menu"; or set menu $examples/../target/release/rmenu
    test -x "$menu"; or begin
        printf 'ghqr: build rmenu first, or set RMENU_BIN\n' >&2
        return 127
    end
    set -l query "$argv"
    set -l dir (command mktemp -d -t ghqr); or return
    function __ghqr_interrupted --on-signal INT -V dir
        functions -e __ghqr_interrupted
        _ghqr_close $dir
    end
    _ghqr_pick $dir $menu $query
    set -l code $status
    set -l result
    test -f $dir/out; and set result (cat $dir/out)
    functions -e __ghqr_interrupted
    _ghqr_close $dir
    test "$code" -eq 0; or return $code

    set -l source_repo
    if printf '%s\n' $result | command jq -e '.source == "memos"' >/dev/null
        printf '%s\n' $result | command jq -e '.path[0].source == "repos"' >/dev/null; or return 1
        set source_repo (_mmenu_single_value "$result" '["path",0]' | string split0); or return 1
    end
    _mmenu_git_dispatch "$result" "$source_repo"
end

function _ghqr_pick --inherit-variable examples --argument-names dir menu query
    command jq -n --arg fish (status fish-path) --arg examples $examples --argjson rex (_mmenu_rex_bindings) '
        {
            sources: [
                {id: "repos", label: "Repositories", prompt: "Repositories", format: "jsonl", command: [$examples + "/repos/repos.sh"]},
                {id: "worktrees", label: "Worktrees", hidden: true, format: "jsonl", command: [$examples + "/repos/worktrees.sh"]},
                {id: "memos", label: "Memos", hidden: true, format: "jsonl", command: [$fish, "--no-config", "-c", "source $argv[1]; _mmenu_memo_rows \"$MMENU_VALUE\"", $examples + "/_mmenu_git.fish"]}
            ],
            bindings: [
                {key: "ctrl+w", id: "worktrees", label: "Worktrees", action: "push", source: "worktrees", sources: ["repos"]},
                {key: "alt+n", id: "memos", label: "Memos", action: "push", source: "memos", sources: ["repos"]}
            ] + $rex
        }' >$dir/config
    or return 1

    if _mmenu_in_rex
        set -l run (status fish-path) --no-config $examples/_ghqr_run.fish \
            $dir $menu --config $dir/config --json --query=$query
        set -l caller_window (rex session inspect $REX_SESSION --json | command jq -r --arg b $REX_BLOCK \
            'first(.windows[] | select(any(.layers[].blocks[]; .block_id == $b)) | .window_id) // empty')
        set -l request (command jq -nc --arg s $REX_SESSION --arg w "$caller_window" --arg cwd $PWD --args '{
            session_id: $s, bounds: [0.24, 0.16, 0.52, 0.48], focus: true}
            + (if $w == "" then {} else {window_id: $w} end) + {
            layout: {block: {flavor: "com.superlogical.terminal.shell", label: "ghqr",
                             options: {command: $ARGS.positional, cwd: $cwd}}}}' -- $run)
        set -l created (rex api call -s $REX_SESSION session.new_layer $request)
        or return 1
        printf '%s\n' $created | command jq -r .layer_id >$dir/layer
        rex wait (printf '%s\n' $created | command jq -r '.block_ids[0]') >/dev/null
        set -l waited $status
        if not test -s $dir/status
            printf 'ghqr: the picker layer ended without a result (rex wait exited %s)\n' $waited >&2
            return 1
        end
        test -s $dir/err; and cat $dir/err >&2
        return (cat $dir/status)
    else
        command $menu --config $dir/config --json --query=$query >$dir/out
    end
end

function _ghqr_close --argument-names dir
    if test -s $dir/layer
        # Rex wait can return while the picker layer still has focus.
        # The layer may already be gone, because its shell exits with rmenu.
        set -l error (rex api call -s $REX_SESSION session.close_layer (command jq -nc --arg s $REX_SESSION --arg l (cat $dir/layer) \
            '{session_id: $s, layer_id: $l}') 2>&1 >/dev/null)
        or string match -q -- '*code=not_found*' $error
        or printf '%s\n' $error >&2
        rex api call -s $REX_SESSION session.focus_block (command jq -nc --arg s $REX_SESSION --arg b $REX_BLOCK \
            '{session_id: $s, block_id: $b}') >/dev/null
    end
    command rm -rf -- $dir
end
