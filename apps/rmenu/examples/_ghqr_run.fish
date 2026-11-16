set -l dir $argv[1]
set -e argv[1]
command $argv >$dir/out 2>$dir/err
printf '%s\n' $status >$dir/status
