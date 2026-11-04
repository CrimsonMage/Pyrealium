#!/bin/sh
# File size rule (README "Build and verify"): plan a split at 1000 lines, fail at 1500.
status=0
for f in $(git ls-files '*.rs'); do
  n=$(wc -l < "$f")
  if [ "$n" -ge 1500 ]; then echo "FAIL $n $f"; status=1
  elif [ "$n" -ge 1000 ]; then echo "plan a split: $n $f"; fi
done
exit $status
