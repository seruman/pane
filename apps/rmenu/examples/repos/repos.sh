#!/bin/sh
roots=$(ghq root --all) || exit
paths=$(ghq list -p) || exit
printf '%s\n' "$paths" | jq -Rc --arg roots "$roots" '
	($roots | split("\n") | map(select(length > 0))) as $roots |
	select(length > 0) | . as $path |
	([$roots[] | select(. as $root | $path | startswith($root + "/"))] | max_by(length)) as $root |
	(if $root then $path[($root | length) + 1:] else $path end) as $name |
	{id: $name, title: $name, value: $path}
'
