/*
 * Exact resource-coalition sampler for the macOS parity benchmark.
 *
 * A process can disappear at any sampling point. Such processes are omitted.
 * If BSD identity establishes that a process has our effective UID but a later
 * query is denied or otherwise unreadable, its PID is reported in
 * unreadableSameUidPids. No ownership is inferred when BSD identity itself is
 * unreadable. Stable start times are checked around each sample to reject PID
 * reuse. This deliberately has no PPID-based fallback.
 */

#ifdef __APPLE__

#include <errno.h>
#include <inttypes.h>
#include <libproc.h>
#include <mach/mach_time.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/proc_info.h>
#include <sys/resource.h>
#include <sys/types.h>
#include <unistd.h>

/* Compatible definitions from xnu's private proc_info_private.h. */
#define CAPTURES_PROC_PIDCOALITIONINFO 20
#define CAPTURES_COALITION_TYPE_RESOURCE 0
struct captures_proc_pidcoalitioninfo {
    uint64_t coalition_id[2];
    uint64_t reserved1;
    uint64_t reserved2;
};

struct process_sample {
    pid_t pid;
    pid_t parent;
    uid_t uid;
    char name[2 * MAXCOMLEN];
    uint64_t start_mach;
    uint64_t user_ns;
    uint64_t system_ns;
    uint64_t resident;
    uint64_t footprint;
    uint64_t lifetime_peak_footprint;
};

struct samples {
    struct process_sample *items;
    size_t count;
    size_t capacity;
    pid_t *unreadable;
    size_t unreadable_count;
    size_t unreadable_capacity;
};

static mach_timebase_info_data_t g_timebase;

static bool disappeared(int error) { return error == ESRCH || error == ENOENT; }

static uint64_t mach_ticks_to_ns(uint64_t ticks) {
    __uint128_t value = (__uint128_t)ticks * g_timebase.numer / g_timebase.denom;
    return value > UINT64_MAX ? UINT64_MAX : (uint64_t)value;
}

static uint64_t timeval_to_ns(struct timeval value) {
    return (uint64_t)value.tv_sec * UINT64_C(1000000000) +
           (uint64_t)value.tv_usec * UINT64_C(1000);
}

static bool cpu_deltas_close(uint64_t proc_delta, uint64_t rusage_delta) {
    uint64_t larger = proc_delta > rusage_delta ? proc_delta : rusage_delta;
    uint64_t difference = proc_delta > rusage_delta ? proc_delta - rusage_delta
                                                     : rusage_delta - proc_delta;
    /* Allow for the differently ordered queries and timer/accounting granularity. */
    return difference <= UINT64_C(10000000) + larger / 5;
}

static int coalition_for_pid(pid_t pid, uint64_t *coalition) {
    struct captures_proc_pidcoalitioninfo info;
    errno = 0;
    int result = proc_pidinfo(pid, CAPTURES_PROC_PIDCOALITIONINFO, 0, &info,
                              (int)sizeof(info));
    if (result != (int)sizeof(info)) {
        if (result >= 0 && errno == 0) errno = EIO;
        return -1;
    }
    *coalition = info.coalition_id[CAPTURES_COALITION_TYPE_RESOURCE];
    return 0;
}

static int bsd_for_pid(pid_t pid, struct proc_bsdinfo *bsd) {
    errno = 0;
    int result = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, bsd, (int)sizeof(*bsd));
    if (result != (int)sizeof(*bsd)) {
        if (result >= 0 && errno == 0) errno = EIO;
        return -1;
    }
    return 0;
}

static int rusage_for_pid(pid_t pid, struct rusage_info_v4 *usage) {
    errno = 0;
    if (proc_pid_rusage(pid, RUSAGE_INFO_V4, (rusage_info_t *)usage) != 0) {
        if (errno == 0) errno = EIO;
        return -1;
    }
    return 0;
}

static int append_sample(struct samples *all, const struct process_sample *sample) {
    if (all->count == all->capacity) {
        size_t capacity = all->capacity ? all->capacity * 2 : 32;
        void *items = realloc(all->items, capacity * sizeof(*all->items));
        if (!items) return -1;
        all->items = items;
        all->capacity = capacity;
    }
    all->items[all->count++] = *sample;
    return 0;
}

static int append_unreadable(struct samples *all, pid_t pid) {
    for (size_t i = 0; i < all->unreadable_count; ++i)
        if (all->unreadable[i] == pid) return 0;
    if (all->unreadable_count == all->unreadable_capacity) {
        size_t capacity = all->unreadable_capacity ? all->unreadable_capacity * 2 : 16;
        void *items = realloc(all->unreadable, capacity * sizeof(*all->unreadable));
        if (!items) return -1;
        all->unreadable = items;
        all->unreadable_capacity = capacity;
    }
    all->unreadable[all->unreadable_count++] = pid;
    return 0;
}

/* Returns 1 for a sample, 0 for a race/non-member, and -1 for unreadable. */
static int sample_pid(pid_t pid, uid_t expected_uid, uint64_t expected_coalition,
                      struct process_sample *sample, bool *same_uid) {
    struct proc_bsdinfo bsd, bsd_after;
    struct rusage_info_v4 before, after;
    uint64_t coalition;
    *same_uid = false;
    if (bsd_for_pid(pid, &bsd) != 0) return disappeared(errno) ? 0 : -1;
    if (bsd.pbi_pid != (uint32_t)pid || bsd.pbi_uid != expected_uid) return 0;
    *same_uid = true;
    if (rusage_for_pid(pid, &before) != 0) return disappeared(errno) ? 0 : -1;
    if (coalition_for_pid(pid, &coalition) != 0) return disappeared(errno) ? 0 : -1;
    if (coalition != expected_coalition) return 0;

    memset(sample, 0, sizeof(*sample));
    sample->pid = pid;
    sample->parent = (pid_t)bsd.pbi_ppid;
    sample->uid = bsd.pbi_uid;
    errno = 0;
    int name_length = proc_name(pid, sample->name, (uint32_t)sizeof(sample->name));
    if (name_length <= 0) return disappeared(errno) ? 0 : -1;
    sample->name[sizeof(sample->name) - 1] = '\0';

    if (rusage_for_pid(pid, &after) != 0) return disappeared(errno) ? 0 : -1;
    if (bsd_for_pid(pid, &bsd_after) != 0) return disappeared(errno) ? 0 : -1;
    if (before.ri_proc_start_abstime != after.ri_proc_start_abstime ||
        bsd.pbi_start_tvsec != bsd_after.pbi_start_tvsec ||
        bsd.pbi_start_tvusec != bsd_after.pbi_start_tvusec ||
        bsd_after.pbi_pid != (uint32_t)pid || bsd_after.pbi_uid != expected_uid)
        return 0;
    sample->start_mach = after.ri_proc_start_abstime;
    /* XNU's own proc_pid_rusage tests convert these Mach ticks to nanoseconds. */
    sample->user_ns = mach_ticks_to_ns(after.ri_user_time);
    sample->system_ns = mach_ticks_to_ns(after.ri_system_time);
    sample->resident = after.ri_resident_size;
    sample->footprint = after.ri_phys_footprint;
    sample->lifetime_peak_footprint = after.ri_lifetime_max_phys_footprint;
    return 1;
}

static int pid_compare(const void *left, const void *right) {
    pid_t a = *(const pid_t *)left, b = *(const pid_t *)right;
    return (a > b) - (a < b);
}

static int sample_compare(const void *left, const void *right) {
    const struct process_sample *a = left, *b = right;
    return (a->pid > b->pid) - (a->pid < b->pid);
}

static int collect(pid_t root, uid_t uid, uint64_t coalition, struct samples *all) {
    int bytes = proc_listpids(PROC_ALL_PIDS, 0, NULL, 0);
    if (bytes <= 0) return -1;
    size_t capacity = (size_t)bytes / sizeof(pid_t) + 128;
    pid_t *pids = calloc(capacity, sizeof(*pids));
    if (!pids) return -1;
    size_t buffer_bytes = capacity * sizeof(*pids);
    if (buffer_bytes > INT32_MAX) { free(pids); errno = EOVERFLOW; return -1; }
    bytes = proc_listpids(PROC_ALL_PIDS, 0, pids, (int)buffer_bytes);
    if (bytes < 0) { free(pids); return -1; }
    if ((size_t)bytes >= buffer_bytes || bytes % (int)sizeof(*pids) != 0) {
        free(pids);
        errno = EOVERFLOW;
        return -1;
    }
    size_t count = (size_t)bytes / sizeof(*pids);

    bool root_seen = false;
    for (size_t i = 0; i < count; ++i) {
        if (pids[i] <= 0) continue;
        struct process_sample sample;
        bool same_uid;
        int result = sample_pid(pids[i], uid, coalition, &sample, &same_uid);
        if (result == 1) {
            if (append_sample(all, &sample) != 0) { free(pids); return -1; }
            if (sample.pid == root) root_seen = true;
        } else if (result < 0 && same_uid) {
            if (append_unreadable(all, pids[i]) != 0) { free(pids); return -1; }
        }
    }
    free(pids);
    if (!root_seen) { errno = ESRCH; return -1; }
    if (all->count > 1)
        qsort(all->items, all->count, sizeof(*all->items), sample_compare);
    if (all->unreadable_count > 1)
        qsort(all->unreadable, all->unreadable_count, sizeof(*all->unreadable), pid_compare);
    return 0;
}

static void json_string(const char *text) {
    putchar('"');
    for (const unsigned char *p = (const unsigned char *)text; *p; ++p) {
        switch (*p) {
            case '"': fputs("\\\"", stdout); break;
            case '\\': fputs("\\\\", stdout); break;
            case '\b': fputs("\\b", stdout); break;
            case '\f': fputs("\\f", stdout); break;
            case '\n': fputs("\\n", stdout); break;
            case '\r': fputs("\\r", stdout); break;
            case '\t': fputs("\\t", stdout); break;
            default:
                if (*p < 0x20 || *p >= 0x7f) printf("\\u%04x", (unsigned)*p);
                else putchar(*p);
        }
    }
    putchar('"');
}

static int run_probe(pid_t root) {
    uint64_t started = mach_absolute_time();
    struct proc_bsdinfo root_bsd;
    struct rusage_info_v4 root_usage;
    uint64_t root_coalition, sampler_coalition;
    if (bsd_for_pid(root, &root_bsd) != 0 || rusage_for_pid(root, &root_usage) != 0 ||
        coalition_for_pid(root, &root_coalition) != 0) {
        fprintf(stderr, "resources: cannot read root PID %d: %s\n", root, strerror(errno));
        return 1;
    }
    if (root_bsd.pbi_pid != (uint32_t)root) {
        fputs("resources: root PID identity mismatch\n", stderr);
        return 1;
    }
    if (root_bsd.pbi_uid != geteuid()) {
        fprintf(stderr, "resources: root PID %d is not owned by effective UID %u\n",
                root, (unsigned)geteuid());
        return 1;
    }
    if (coalition_for_pid(getpid(), &sampler_coalition) != 0) {
        fprintf(stderr, "resources: cannot read sampler coalition: %s\n", strerror(errno));
        return 1;
    }

    struct samples all = {0};
    if (collect(root, geteuid(), root_coalition, &all) != 0) {
        fprintf(stderr, "resources: failed to sample root coalition: %s\n", strerror(errno));
        free(all.items); free(all.unreadable);
        return 1;
    }
    bool root_identity_stable = false;
    for (size_t i = 0; i < all.count; ++i) {
        if (all.items[i].pid == root &&
            all.items[i].start_mach == root_usage.ri_proc_start_abstime) {
            root_identity_stable = true;
            break;
        }
    }
    if (!root_identity_stable) {
        fputs("resources: root PID changed identity during sampling\n", stderr);
        free(all.items); free(all.unreadable);
        return 1;
    }
    uint64_t finished = mach_absolute_time();
    printf("{\"schema\":1,\"rootPid\":%d,\"rootStartMach\":%" PRIu64
           ",\"resourceCoalitionId\":%" PRIu64 ",\"samplerCoalitionId\":%" PRIu64
           ",\"hostTimeNs\":%" PRIu64 ",\"elapsedProbeNs\":%" PRIu64 ",\"processes\":[",
           root, root_usage.ri_proc_start_abstime, root_coalition, sampler_coalition,
           mach_ticks_to_ns(finished), mach_ticks_to_ns(finished - started));
    for (size_t i = 0; i < all.count; ++i) {
        const struct process_sample *p = &all.items[i];
        if (i) putchar(',');
        printf("{\"pid\":%d,\"parent\":%d,\"uid\":%u,\"name\":", p->pid, p->parent,
               (unsigned)p->uid);
        json_string(p->name);
        printf(",\"startMach\":%" PRIu64 ",\"userCpuNs\":%" PRIu64
               ",\"systemCpuNs\":%" PRIu64 ",\"residentBytes\":%" PRIu64
               ",\"physicalFootprintBytes\":%" PRIu64
               ",\"lifetimePeakPhysicalFootprintBytes\":%" PRIu64 "}",
               p->start_mach, p->user_ns, p->system_ns, p->resident, p->footprint,
               p->lifetime_peak_footprint);
    }
    fputs("],\"unreadableSameUidPids\":[", stdout);
    for (size_t i = 0; i < all.unreadable_count; ++i) {
        if (i) putchar(',');
        printf("%d", all.unreadable[i]);
    }
    fputs("]}\n", stdout);
    free(all.items); free(all.unreadable);
    return ferror(stdout) ? 1 : 0;
}

static int self_test(void) {
    if (mach_timebase_info(&g_timebase) != KERN_SUCCESS || g_timebase.denom == 0) {
        fputs("resources self-test: invalid Mach timebase\n", stderr); return 1;
    }
    pid_t self = getpid();
    struct proc_bsdinfo bsd;
    struct rusage_info_v4 usage;
    uint64_t coalition, coalition_again;
    if (bsd_for_pid(self, &bsd) != 0 || rusage_for_pid(self, &usage) != 0 ||
        coalition_for_pid(self, &coalition) != 0 || coalition_for_pid(self, &coalition_again) != 0) {
        fprintf(stderr, "resources self-test: cannot read own identity: %s\n", strerror(errno));
        return 1;
    }
    if (bsd.pbi_pid != (uint32_t)self || bsd.pbi_uid != geteuid() ||
        usage.ri_proc_start_abstime == 0 || usage.ri_resident_size == 0 ||
        usage.ri_phys_footprint == 0 || usage.ri_lifetime_max_phys_footprint < usage.ri_phys_footprint ||
        coalition == 0 || coalition != coalition_again ||
        mach_ticks_to_ns(mach_absolute_time()) == 0) {
        fputs("resources self-test: identity, coalition, or resource counters are invalid\n", stderr);
        return 1;
    }

    struct rusage_info_v4 proc_before, proc_after;
    struct rusage rusage_before, rusage_after;
    if (rusage_for_pid(self, &proc_before) != 0 ||
        getrusage(RUSAGE_SELF, &rusage_before) != 0) {
        fprintf(stderr, "resources self-test: cannot start CPU cross-check: %s\n",
                strerror(errno));
        return 1;
    }
    uint64_t busy_started = mach_absolute_time();
    uint64_t busy_duration = g_timebase.denom * UINT64_C(150000000) / g_timebase.numer;
    volatile uint64_t work = UINT64_C(0x9e3779b97f4a7c15);
    uint64_t iterations = 0;
    while (mach_absolute_time() - busy_started < busy_duration) {
        work ^= work << 7;
        work ^= work >> 9;
        work *= UINT64_C(0xbf58476d1ce4e5b9);
        if ((++iterations & UINT64_C(0x3fff)) == 0) (void)getpid();
    }
    if (getrusage(RUSAGE_SELF, &rusage_after) != 0 ||
        rusage_for_pid(self, &proc_after) != 0) {
        fprintf(stderr, "resources self-test: cannot finish CPU cross-check: %s\n",
                strerror(errno));
        return 1;
    }
    if (proc_after.ri_user_time < proc_before.ri_user_time ||
        proc_after.ri_system_time < proc_before.ri_system_time) {
        fputs("resources self-test: CPU counters went backwards\n", stderr);
        return 1;
    }
    uint64_t proc_user_ns = mach_ticks_to_ns(proc_after.ri_user_time - proc_before.ri_user_time);
    uint64_t proc_system_ns =
        mach_ticks_to_ns(proc_after.ri_system_time - proc_before.ri_system_time);
    uint64_t rusage_user_ns = timeval_to_ns(rusage_after.ru_utime) -
                              timeval_to_ns(rusage_before.ru_utime);
    uint64_t rusage_system_ns = timeval_to_ns(rusage_after.ru_stime) -
                                timeval_to_ns(rusage_before.ru_stime);
    if (proc_user_ns == 0 ||
        !cpu_deltas_close(proc_user_ns, rusage_user_ns) ||
        !cpu_deltas_close(proc_system_ns, rusage_system_ns)) {
        fprintf(stderr,
                "resources self-test: CPU cross-check failed "
                "(proc user/system=%" PRIu64 "/%" PRIu64
                " ns, getrusage user/system=%" PRIu64 "/%" PRIu64 " ns)\n",
                proc_user_ns, proc_system_ns, rusage_user_ns, rusage_system_ns);
        return 1;
    }
    struct process_sample sample;
    bool same_uid = false;
    if (sample_pid(self, geteuid(), coalition, &sample, &same_uid) != 1 || !same_uid ||
        sample.pid != self || sample.start_mach != usage.ri_proc_start_abstime) {
        fputs("resources self-test: stable self sample failed\n", stderr); return 1;
    }
    puts("resources self-test: ok");
    return 0;
}

int main(int argc, char **argv) {
    if (mach_timebase_info(&g_timebase) != KERN_SUCCESS || g_timebase.denom == 0) {
        fputs("resources: mach_timebase_info failed\n", stderr); return 1;
    }
    if (argc == 2 && strcmp(argv[1], "--self-test") == 0) return self_test();
    if (argc != 2 || argv[1][0] == '\0' || argv[1][0] == '-') {
        fputs("usage: resources ROOT_PID | resources --self-test\n", stderr); return 2;
    }
    char *end = NULL;
    errno = 0;
    long value = strtol(argv[1], &end, 10);
    if (errno != 0 || *end != '\0' || value <= 0 || value > INT32_MAX) {
        fputs("resources: ROOT_PID must be a positive process ID\n", stderr); return 2;
    }
    return run_probe((pid_t)value);
}

#else

#include <errno.h>
#include <limits.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(int argc, char **argv) {
    if (argc == 2 && strcmp(argv[1], "--self-test") == 0) {
        fputs("resources self-test: requires macOS libproc and Mach APIs\n", stderr);
        return 1;
    }
    if (argc != 2) {
        fputs("usage: resources ROOT_PID | resources --self-test\n", stderr);
        return 2;
    }
    char *end = NULL;
    errno = 0;
    long value = strtol(argv[1], &end, 10);
    if (errno != 0 || argv[1][0] == '\0' || *end != '\0' || value <= 0 || value > INT_MAX) {
        fputs("resources: ROOT_PID must be a positive process ID\n", stderr);
        return 2;
    }
    fputs("resources: native coalition probing is only available on macOS\n", stderr);
    return 1;
}

#endif
