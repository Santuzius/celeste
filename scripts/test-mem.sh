#!/usr/bin/env bash
# Prints PSS / RSS (MB) of the Syncative process of the isolated test instance (HOME=/tmp/syncative-test/home); never looks at the real instance.
for pid in $(pgrep -u "$USER" -f 'target/(debug|release)/syncative'); do
  if { tr '\0' '\n' < "/proc/$pid/environ"; } 2>/dev/null | grep -qx "HOME=/tmp/syncative-test/home"; then
    awk -v pid="$pid" '/^Pss:/{p=$2} /^Rss:/{r=$2} END{printf "pid %s  PSS %.0f MB  RSS %.0f MB\n", pid, p/1024, r/1024}' "/proc/$pid/smaps_rollup"
  fi
done
