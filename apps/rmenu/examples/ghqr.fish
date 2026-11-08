# ghqm drawn by rmenu in a Rex floating layer: source this file, then `ghqr [query]`.
# Same levels and keys as mmenu's examples/ghqm.fish. Outside Rex it draws in this terminal.
source (path dirname -- (status filename))/_mmenu_git.fish

function ghqr --description 'Pick a ghq repository, worktree or memo in rmenu, in a Rex floating layer'
    _mmenu_require ghqr ghq git jq; or return
    set -l menu $RMENU_BIN
    test -n "$menu"; or set menu (path resolve -- (path dirname -- (status filename))/../target/release/rmenu)
    test -x "$menu"; or begin; printf 'ghqr: build rmenu first, or set RMENU_BIN\n' >&2; return 127; end
    set -l query (string join ' ' -- $argv)
    # One private directory for the config and everything the picker hands back.
    set -l dir (command mktemp -d -t ghqr); or return
    command jq -n --arg fish (status fish-path) --arg helper (path resolve -- (path dirname -- (status filename))/_mmenu_git.fish) \
        --argjson rex (_mmenu_rex_bindings) '
        def rows($call): [$fish, "--no-config", "-c", "source $argv[1]; " + $call, $helper];
        {
            sources: [
                {id: "repos", label: "Repositories", prompt: "Repositories", format: "jsonl", command: rows("_mmenu_repo_rows")},
                {id: "worktrees", label: "Worktrees", hidden: true, format: "jsonl", command: rows("_mmenu_worktree_rows \"$MMENU_VALUE\"")},
                {id: "memos", label: "Memos", hidden: true, format: "jsonl", command: rows("_mmenu_memo_rows \"$MMENU_VALUE\"")}
            ],
            bindings: [
                {key: "ctrl+w", id: "worktrees", label: "Worktrees", action: "push", source: "worktrees", sources: ["repos"]},
                {key: "alt+n", id: "memos", label: "Memos", action: "push", source: "memos", sources: ["repos"]}
            ] + $rex
        }' >$dir/config
    or begin; command rm -rf -- $dir; return 1; end

    set -l code
    if test "$TERM_PROGRAM" = rex; and set -q REX_SESSION REX_BLOCK; and command -q rex
        # The layer's shell runs the picker and leaves its output, errors and status in $dir.
        set -l run (status fish-path) --no-config -c 'set -l dir $argv[1]; set -e argv[1]; $argv >$dir/out 2>$dir/err; echo $status >$dir/status' \
            $dir $menu --config $dir/config --json --query=$query
        # Over the window this shell is in, not whichever window is active.
        set -l window (rex session inspect $REX_SESSION --json | command jq -r --arg b $REX_BLOCK \
            'first(.windows[] | select(any(.layers[].blocks[]; .block_id == $b)) | .window_id) // empty')
        set -l request (command jq -nc --arg s $REX_SESSION --arg w "$window" --arg cwd $PWD --args '{
            session_id: $s, bounds: [0.24, 0.16, 0.52, 0.48], focus: true}
            + (if $w == "" then {} else {window_id: $w} end) + {
            layout: {block: {flavor: "com.superlogical.terminal.shell", label: "ghqr",
                             options: {command: $ARGS.positional, cwd: $cwd}}}}' -- $run)
        set -l created (rex api call -s $REX_SESSION session.new_layer $request)
        or begin; command rm -rf -- $dir; return 1; end
        set -l layer (printf '%s\n' $created | command jq -r .layer_id)
        rex wait (printf '%s\n' $created | command jq -r '.block_ids[0]') >/dev/null
        set -l waited $status
        # wait returns when rmenu exits, while the layer may still hold focus. Close it and
        # focus this shell's block, so a split or tab is anchored here, not in the layer.
        rex api call -s $REX_SESSION session.close_layer (command jq -nc --arg s $REX_SESSION --arg l $layer \
            '{session_id: $s, layer_id: $l}') >/dev/null 2>&1
        rex api call -s $REX_SESSION session.focus_block (command jq -nc --arg s $REX_SESSION --arg b $REX_BLOCK \
            '{session_id: $s, block_id: $b}') >/dev/null 2>&1
        if test -s $dir/status
            set code (cat $dir/status)
        else
            # No status: the picker never ran, or the layer was closed under it.
            printf 'ghqr: the picker layer ended without a result (rex wait exited %s)\n' $waited >&2
            command rm -rf -- $dir
            return 1
        end
        test -s $dir/err; and cat $dir/err >&2
    else
        command $menu --config $dir/config --json --query=$query >$dir/out
        set code $status
    end
    set -l result (cat $dir/out)
    command rm -rf -- $dir
    test "$code" -eq 0; or return $code

    set -l source_repo
    if printf '%s\n' $result | command jq -e '.source == "memos"' >/dev/null
        printf '%s\n' $result | command jq -e '.path[0].source == "repos"' >/dev/null; or return 1
        set source_repo (_mmenu_single_value "$result" '["path",0]' | string split0); or return 1
    end
    _mmenu_git_dispatch "$result" "$source_repo"
end
