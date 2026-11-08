# ghqm drawn by rmenu in a Rex floating layer: source this file, then `ghqr [query]`.
# Same levels and keys as mmenu's examples/ghqm.fish. Outside Rex it draws in this terminal.
source (path dirname -- (status filename))/_mmenu_git.fish

function ghqr --description 'Pick a ghq repository, worktree or memo in rmenu, in a Rex floating layer'
    _mmenu_require ghqr ghq git jq; or return
    set -l menu $RMENU_BIN
    test -n "$menu"; or set menu (path resolve -- (path dirname -- (status function-file 2>/dev/null; or status filename))/../target/release/rmenu)
    test -x "$menu"; or begin; printf 'ghqr: build rmenu first, or set RMENU_BIN\n' >&2; return 127; end
    set -l query (string join ' ' -- $argv)
    set -l config (command mktemp -t ghqr.XXXXXX); or return
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
        }' >$config
    or begin; command rm -f -- $config; return 1; end

    set -l out (command mktemp -t ghqr-out.XXXXXX)
    set -l code
    if test "$TERM_PROGRAM" = rex; and set -q REX_SESSION; and command -q rex
        # The layer's shell writes the result and status where this shell can read them.
        set -l run (status fish-path) --no-config -c 'set -l out $argv[1]; set -e argv[1]; $argv >$out; echo $status >$out.status' \
            $out $menu --config $config --json --query=$query
        # Over the window this shell is in, not whichever window is active.
        set -l window (rex session inspect $REX_SESSION --json | command jq -r --arg b "$REX_BLOCK" \
            'first(.windows[] | select(any(.layers[].blocks[]; .block_id == $b)) | .window_id) // empty')
        set -l request (command jq -nc --arg s $REX_SESSION --arg w "$window" --arg cwd $PWD --args '{
            session_id: $s, bounds: [0.18, 0.12, 0.64, 0.62], focus: true}
            + (if $w == "" then {} else {window_id: $w} end) + {
            layout: {block: {flavor: "com.superlogical.terminal.shell", label: "ghqr",
                             options: {command: $ARGS.positional, cwd: $cwd}}}}' -- $run)
        set -l block (rex api call -s $REX_SESSION session.new_layer $request | command jq -r '.block_ids[0]')
        or begin; command rm -f -- $config $out; return 1; end
        rex wait $block >/dev/null
        set code (cat $out.status 2>/dev/null; or echo 130)
    else
        command $menu --config $config --json --query=$query >$out
        set code $status
    end
    set -l result (cat $out)
    command rm -f -- $config $out $out.status
    test "$code" -eq 0; or return $code

    set -l source_repo
    if printf '%s\n' $result | command jq -e '.source == "memos"' >/dev/null
        printf '%s\n' $result | command jq -e '.path[0].source == "repos"' >/dev/null; or return 1
        set source_repo (_mmenu_single_value "$result" '["path",0]' | string split0); or return 1
    end
    _mmenu_git_dispatch "$result" "$source_repo"
end
