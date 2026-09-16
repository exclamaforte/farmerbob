# fb-sem — a counting semaphore over flock, for the verification workload.
#
# Verification was serialised behind a single lock after two OOMs. That was an
# overcorrection: the OOMs came from FOUR UNCONFINED rustc processes alongside agents
# budgeted at 6G each on an 18G machine. Measured properly, one cold cargo cycle peaks at
# ~881M across all rustc+cargo processes, so a 17G machine supports many.
#
# Verification is cpu/memory-bound while agents are network-bound (~5% cpu in
# do_epoll_wait), so the two workloads have inverted profiles and should be limited
# independently.   (beads farmerbob-89j, farmerbob-4ix)
#
#   fb_sem_run <slots> <lockdir> <cmd...>
fb_sem_run() {
  local slots="${1:?slots}" dir="${2:?lockdir}"; shift 2
  mkdir -p "$dir"
  local i fd
  while :; do
    for i in $(seq 1 "$slots"); do
      exec {fd}>"$dir/slot.$i"
      if flock -n "$fd"; then
        "$@"; local rc=$?
        flock -u "$fd"; exec {fd}>&-
        return $rc
      fi
      exec {fd}>&-
    done
    sleep 2
  done
}
