/* logi-rpm-bridge: forward the game telemetry relay's RPM to the driver.
 *
 * Listens for LTFR datagrams on 127.0.0.1:20780 (the dinput8 escape
 * proxy's telemetry relay) and writes "rpm max_rpm" to the wheel's
 * wheel_texture_rpm sysfs attribute, which feeds the native texture
 * merge. When the datagram carries the appended first-shift-light rpm
 * (32-byte form; the game's telemetry triple is rpm / first-led /
 * redline), it also drives the rev strip through wheel_rev_level:
 * dark below the car's own first-led rpm, filling linearly to all ten
 * levels at the limiter - the same mapping G HUB applies on Windows.
 * Exits 0 on SIGTERM/SIGINT. Build:
 *   cc -O2 -Wall -o logi-rpm-bridge logi-rpm-bridge.c
 */
#include <arpa/inet.h>
#include <errno.h>
#include <glob.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/time.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t stop;
static void on_sig(int s) { (void)s; stop = 1; }

static const char *find_sysfs(char *buf, size_t n)
{
	const char *env = getenv("LOGI_RPM_SYSFS");
	glob_t g;

	if (env && *env)
		return env;
	if (!glob("/sys/bus/hid/devices/*046D:C2*/wheel_texture_rpm",
		  0, NULL, &g) && g.gl_pathc) {
		snprintf(buf, n, "%s", g.gl_pathv[0]);
		globfree(&g);
		return buf;
	}
	globfree(&g);
	return NULL;
}

/* The rev strip lives next door to the texture attribute. */
static const char *find_rev_sysfs(const char *rpm_path, char *buf, size_t n)
{
	const char *env = getenv("LOGI_REV_SYSFS");
	char dir[256];
	const char *slash;

	if (env && *env)
		return env;
	slash = strrchr(rpm_path, '/');
	if (!slash || (size_t)(slash - rpm_path) >= sizeof(dir))
		return NULL;
	memcpy(dir, rpm_path, slash - rpm_path);
	dir[slash - rpm_path] = '\0';
	if ((size_t)snprintf(buf, n, "%s/wheel_rev_level", dir) >= n)
		return NULL;
	return buf;
}

/* Two mappings, chosen by LOGI_REV_MODE:
 *
 * "bar" (default): a full-range rev bar - LED 1 as soon as the engine
 * turns, all 10 at the limiter. Needs only rpm+max_rpm, so it also works
 * for legacy 28-byte senders that don't carry the first-light field.
 *
 * "shift": G HUB's dash mapping - dark below the car's own
 * first-shift-light rpm, level 1 exactly there, all 10 at the limiter.
 * Needs the 32-byte datagram; without a sane triple the packet's LED
 * data is treated as absent (-1 = leave LEDs alone). */
static int rev_level(float rpm, float first_led, float max_rpm, int shift_mode)
{
	float base, span;
	int level;

	if (shift_mode) {
		if (!(first_led > 0.0f) || !(max_rpm > first_led))
			return -1;
		base = first_led;
	} else {
		if (!(max_rpm > 0.0f))
			return -1;
		base = 0.0f;
	}
	/* Engine off is dark in both modes; in shift mode the band opens AT
	 * first_led (level 1 exactly there, per the G HUB captures). */
	if (rpm <= 0.0f || rpm < base)
		return 0;
	span = max_rpm - base;
	level = 1 + (int)(9.0f * (rpm - base) / span);
	if (level > 10)
		level = 10;
	return level;
}

/* The wheel's two attribute paths, resolved together and re-resolved
 * together: the rev path is derived from the rpm path's directory, so one
 * without the other is a pair that can disagree about which wheel it means. */
struct target {
	char rpm_buf[256];
	char rev_buf[256];
	const char *rpm;	/* NULL when no wheel is attached */
	const char *rev;	/* NULL when the wheel exposes no rev strip */
};

static void resolve_target(struct target *t)
{
	t->rpm = find_sysfs(t->rpm_buf, sizeof(t->rpm_buf));
	t->rev = t->rpm ? find_rev_sysfs(t->rpm, t->rev_buf, sizeof(t->rev_buf))
			: NULL;
}

/* Returns 0 on success, -1 with errno set. */
static int write_attr(const char *path, const char *val, time_t *last_warn)
{
	FILE *f = fopen(path, "w");

	if (!f) {
		time_t nowt = time(NULL);

		if (nowt - *last_warn >= 30) {
			fprintf(stderr, "logi-rpm-bridge: cannot write %s: %s\n",
				path, strerror(errno));
			*last_warn = nowt;
		}
		return -1;	/* wheel unplugged; keep listening */
	}
	fputs(val, f);
	fclose(f);
	return 0;
}

/* Whether a failed write means the path no longer names a wheel.
 *
 * The path is resolved once by a glob and then held for the process
 * lifetime, which makes it an identity only until the first replug: hidraw
 * numbering is recycled, so the node this program was started against can
 * come back as a different wheel's, or not come back at all. ENOENT is the
 * attribute file gone with its directory, ENODEV the directory still there
 * with nothing behind it; either way the answer is to look again. */
static int path_went_away(int err)
{
	return err == ENOENT || err == ENODEV;
}

/* Look for the wheel again, at most once a second: an unplugged wheel must
 * not turn a 100 Hz write into a 100 Hz glob. */
static void reresolve(struct target *t, time_t *last_try)
{
	time_t nowt = time(NULL);

	if (nowt == *last_try)
		return;
	*last_try = nowt;
	resolve_target(t);
}

int main(void)
{
	struct target t = { 0 };
	struct sockaddr_in addr = { 0 };
	unsigned char pkt[64];
	struct timespec last = { 0 };
	time_t last_warn = 0, last_rev_warn = 0, last_resolve = 0;
	int last_level = -1;	/* what the strip currently shows */
	const char *mode = getenv("LOGI_REV_MODE");
	int shift_mode = mode && !strcmp(mode, "shift");
	const char *port_env = getenv("LOGI_RPM_PORT");
	int port = port_env && atoi(port_env) > 0 ? atoi(port_env) : 20780;
	int fd;

	resolve_target(&t);
	if (!t.rpm) {
		fprintf(stderr, "logi-rpm-bridge: no wheel_texture_rpm in sysfs\n");
		return 1;
	}
	fd = socket(AF_INET, SOCK_DGRAM, 0);
	if (fd < 0) { perror("socket"); return 1; }
	addr.sin_family = AF_INET;
	addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	addr.sin_port = htons((unsigned short)port);
	/*
	 * Deliberately NO SO_REUSEADDR/SO_REUSEPORT here.
	 *
	 * logi-tf-sim listens for the same LTFR datagrams on the same port
	 * (0.0.0.0:20780, its `port.relay`), and only one of us can have
	 * them: measured on 7.1.x, two UDP sockets on one port deliver a
	 * unicast datagram to exactly ONE socket. With SO_REUSEADDR on both
	 * ends both binds succeed and the kernel hands every packet to the
	 * more specific (or last) bind, so the loser sits on a live socket
	 * that never receives anything. SO_REUSEPORT is worse: it is a
	 * load balancer, so a single producer's packets all hash to one
	 * socket anyway and a second producer would split the stream.
	 *
	 * Without the option the second bind fails with EADDRINUSE, which
	 * is the point: a conflict we can name beats one that is silent.
	 * UDP has no TIME_WAIT, so nothing else wanted the option either.
	 */
	{
		/* Bounded recv so a vanished producer darkens the strip:
		 * without telemetry the game is gone and stale lights lie. */
		struct timeval tv = { .tv_sec = 1, .tv_usec = 0 };

		setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
	}
	if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
		if (errno == EADDRINUSE) {
			/* Almost always logi-tf-sim, which binds the same
			 * port for the same datagrams and is left running
			 * between sessions. Say what is lost and how to get
			 * it back, because the symptom on its own (a flat
			 * texture and a dark rev strip) looks like broken
			 * hardware or a missing telemetry producer.
			 *
			 * Exiting, not idling: this socket is the whole
			 * program, and a pid that stays up doing nothing is
			 * exactly what made the old failure read as
			 * success in logi-launch's log. */
			fprintf(stderr,
				"logi-rpm-bridge: udp/%d is already taken, almost certainly by logi-tf-sim\n"
				"logi-rpm-bridge: it listens on the same port for the same telemetry, and only one of us can have it.\n"
				"logi-rpm-bridge: without it the texture merge gets no rpm (the engine texture stays flat)\n"
				"logi-rpm-bridge: and the rev lights stay dark. Stop logi-tf-sim (pkill -x logi-tf-sim) and\n"
				"logi-rpm-bridge: start the game again, or move one of us with LOGI_RPM_PORT / the daemon's port.relay.\n",
				port);
			return 1;
		}
		perror("bind relay port"); return 1;
	}
	{
		/* sigaction without SA_RESTART: plain signal() on Linux/glibc
		 * defaults to BSD semantics (SA_RESTART set), which makes the
		 * kernel transparently restart recv() across the signal and
		 * the process never observes the interruption. We need recv()
		 * to return EINTR so the loop re-checks stop and exits. */
		struct sigaction sa = { 0 };

		sa.sa_handler = on_sig;
		sigaction(SIGTERM, &sa, NULL);
		sigaction(SIGINT, &sa, NULL);
	}

	while (!stop) {
		ssize_t n = recv(fd, pkt, sizeof(pkt), 0);
		float rpm, max_rpm, first_led = 0.0f;
		struct timespec now;

		if (n < 0) {
			if ((errno == EAGAIN || errno == EWOULDBLOCK) &&
			    t.rev && last_level > 0) {
				write_attr(t.rev, "0", &last_rev_warn);
				last_level = 0;
			}
			continue;
		}
		if (n < 28 || memcmp(pkt, "LTFR", 4) || pkt[4] != 2)
			continue;
		memcpy(&rpm, pkt + 14, 4);
		memcpy(&max_rpm, pkt + 18, 4);
		if (n >= 32)
			memcpy(&first_led, pkt + 28, 4);
		/* Deliberate asymmetry: an out-of-range rpm drops the whole
		 * packet because the sample itself is garbage, while an
		 * out-of-range max_rpm is only clamped below, since a silly
		 * max should not cost us an otherwise good rpm sample. */
		if (!(rpm >= 0.0f && rpm < 30000.0f))
			continue;
		if (max_rpm < 0.0f)
			max_rpm = 0.0f;
		else if (max_rpm >= 30000.0f)
			max_rpm = 29999.0f;
		if (t.rev) {
			int level = rev_level(rpm, first_led, max_rpm,
					      shift_mode);

			if (level >= 0 && level != last_level) {
				char lv[4];

				snprintf(lv, sizeof(lv), "%d", level);
				if (write_attr(t.rev, lv, &last_rev_warn) < 0 &&
				    path_went_away(errno))
					reresolve(&t, &last_resolve);
				last_level = level;
			}
		}
		clock_gettime(CLOCK_MONOTONIC, &now);
		{
			long ns = (now.tv_sec - last.tv_sec) * 1000000000L
				+ (now.tv_nsec - last.tv_nsec);
			if (ns < 10 * 1000 * 1000)
				continue;
		}
		last = now;
		{
			char val[32];

			snprintf(val, sizeof(val), "%.0f %.0f", rpm, max_rpm);
			/* Nothing to write to: the wheel was unplugged, or a
			 * write said its path is gone and the look again found
			 * nothing. Keep looking (rate-limited) so a wheel that
			 * comes back is picked up without a restart. */
			if (!t.rpm)
				reresolve(&t, &last_resolve);
			if (t.rpm && write_attr(t.rpm, val, &last_warn) < 0 &&
			    path_went_away(errno)) {
				reresolve(&t, &last_resolve);
				/* A wheel that came back knows nothing about
				 * the level we last wrote, so the next packet
				 * must write one rather than skip it as
				 * unchanged. */
				last_level = -1;
			}
		}
	}
	if (t.rev && last_level > 0)
		write_attr(t.rev, "0", &last_rev_warn);
	close(fd);
	return 0;
}
