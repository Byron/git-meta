#!/usr/bin/env bash
set -eu -o pipefail

git init -q
mkdir -p dir
printf 'first\nsecond\nthird\nfourth\n' > dir/original.txt
printf 'old\n' > deleted.txt
printf 'plain\n' > executable.txt
printf 'target\n' > typechange.txt
git add .
git commit -qm root
git tag root
printf 'first\nSECOND\nthird\nfourth\n' > dir/original.txt
git commit -qam modified
git mv dir/original.txt dir/renamed.txt
printf 'first\nSECOND\nTHIRD\nfourth\n' > dir/renamed.txt
rm deleted.txt typechange.txt
ln -s executable.txt typechange.txt
chmod +x executable.txt
printf 'new\n' > 'dir/é-tab	.txt'
git add -A
git commit -qm renamed
printf 'first\nSECOND\nTHIRD\nfourth\nuncommitted\n' > dir/renamed.txt

merge=$(git commit-tree HEAD^{tree} -p HEAD -p root -m merge)
git tag merge "$merge"
