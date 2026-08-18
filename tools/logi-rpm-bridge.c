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

/*
 * Reading the relay stream without taking it away from anyone.
 *
 * logi-tf-sim listens for the very same LTFR datagrams on the very same
 * port, because it synthesizes engine haptics from what we use for the
 * texture merge and the rev lights. Only one socket can have them:
 * measured on 7.1.x, a unicast datagram goes to exactly ONE socket however
 * the port is shared. SO_REUSEADDR on both ends only makes both binds
 * succeed while the kernel picks a single winner, which turns the loss
 * silent; SO_REUSEPORT is a load balancer, so it splits a stream rather
 * than duplicating it. The kernel will not deliver to both, so somebody
 * has to.
 *
 * Three rules, the same ones logi-wheel-core's relay::RelayListener
 * implements in Rust. Any change here belongs there too:
 *
 *   1. Take the relay port if it is free. You are then the hub, and you
 *      forward every datagram you receive, verbatim and before parsing it,
 *      to the fan-out ports.
 *   2. If it is taken, read the first free fan-out port instead. The hub
 *      feeds you.
 *   3. As a follower, keep trying to take the relay port. When the hub
 *      exits, the survivor is promoted within a couple of seconds and the
 *      next program to start finds a working hub again.
 *
 * Forwarding only ever goes upward, from the relay port to the fan-out
 * ports, so no arrangement of these programs can make a datagram
 * circulate. base + 1 is skipped because at the default port that is the
 * captured-TrueForce port (20781), where a copy of engine telemetry would
 * be read as finished haptics.
 */
#define FANOUT_PORTS 3
#define PROMOTE_INTERVAL 2

struct relay_in {
	int fd;
	int base;	/* the relay port, whoever holds it */
	int port;	/* the port actually being read */
	int hub;	/* holds the relay port, so forwards to the others */
	struct sockaddr_in fanout[FANOUT_PORTS];
	time_t next_promote;
};

/* A bound UDP socket with the bounded recv the caller needs, or -1 with
 * errno left alone so EADDRINUSE can be told from a real failure. */
static int bind_udp(int port)
{
	struct sockaddr_in addr = { 0 };
	/* Bounded recv so a vanished producer darkens the strip: without
	 * telemetry the game is gone and stale lights lie. It also paces
	 * the promotion attempts, which need no timer of their own. */
	struct timeval tv = { .tv_sec = 1, .tv_usec = 0 };
	int fd = socket(AF_INET, SOCK_DGRAM, 0);

	if (fd < 0)
		return -1;
	addr.sin_family = AF_INET;
	addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	addr.sin_port = htons((unsigned short)port);
	setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
	if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
		int err = errno;

		close(fd);
		errno = err;
		return -1;
	}
	return fd;
}

/* Take the relay port, or the first free fan-out port behind it. Returns 0
 * on success, -1 when every one of them is taken. */
static int relay_open(struct relay_in *r, int base)
{
	int i;

	memset(r, 0, sizeof(*r));
	r->base = base;
	r->fd = -1;
	for (i = 0; i < FANOUT_PORTS; i++) {
		r->fanout[i].sin_family = AF_INET;
		r->fanout[i].sin_addr.s_addr = htonl(INADDR_LOOPBACK);
		r->fanout[i].sin_port = htons((unsigned short)(base + 2 + i));
	}
	for (i = 0; i <= FANOUT_PORTS; i++) {
		int port = i == 0 ? base : base + 1 + i;
		int fd = bind_udp(port);

		if (fd >= 0) {
			r->fd = fd;
			r->port = port;
			r->hub = i == 0;
			r->next_promote = time(NULL) + PROMOTE_INTERVAL;
			return 0;
		}
		if (errno != EADDRINUSE)
			return -1;
	}
	return -1;
}

/* Pass a datagram on to the other readers. Nothing may be listening on a
 * fan-out port, which is the ordinary one-reader case rather than a fault,
 * so send errors are ignored instead of logged sixty times a second. */
static void relay_forward(const struct relay_in *r, const void *pkt, size_t n)
{
	int i;

	if (!r->hub)
		return;
	for (i = 0; i < FANOUT_PORTS; i++)
		(void)!sendto(r->fd, pkt, n, MSG_DONTWAIT,
			      (const struct sockaddr *)&r->fanout[i],
			      sizeof(r->fanout[i]));
}

/* A follower's periodic attempt to take the relay port back. Returns 1 when
 * it succeeded, so the caller can say so. */
static int relay_promote(struct relay_in *r)
{
	time_t nowt = time(NULL);
	int fd;

	if (r->hub || nowt < r->next_promote)
		return 0;
	r->next_promote = nowt + PROMOTE_INTERVAL;
	fd = bind_udp(r->base);
	if (fd < 0)
		return 0;
	/* The fan-out socket goes only once the relay port is held, so this
	 * program is never listening nowhere. It must go, though: a hub
	 * still holding its old fan-out socket would forward every datagram
	 * straight back into itself. */
	close(r->fd);
	r->fd = fd;
	r->port = r->base;
	r->hub = 1;
	return 1;
}

int main(void)
{
	struct target t = { 0 };
	struct relay_in relay;
	unsigned char pkt[64];
	struct timespec last = { 0 };
	time_t last_warn = 0, last_rev_warn = 0, last_resolve = 0;
	int last_level = -1;	/* what the strip currently shows */
	const char *mode = getenv("LOGI_REV_MODE");
	int shift_mode = mode && !strcmp(mode, "shift");
	const char *port_env = getenv("LOGI_RPM_PORT");
	int port = port_env && atoi(port_env) > 0 ? atoi(port_env) : 20780;

	resolve_target(&t);
	if (!t.rpm) {
		fprintf(stderr, "logi-rpm-bridge: no wheel_texture_rpm in sysfs\n");
		return 1;
	}
	if (relay_open(&relay, port) < 0) {
		/* Only when the relay port and every fan-out port behind it
		 * are taken, which means more readers of this stream than
		 * the fan-out was built for. Saying what is lost matters
		 * because the symptom on its own (a flat texture and a dark
		 * rev strip) looks like broken hardware or a missing
		 * telemetry producer.
		 *
		 * Exiting, not idling: this socket is the whole program,
		 * and a pid that stays up doing nothing is exactly what
		 * made the old failure read as success in logi-launch's
		 * log. */
		fprintf(stderr,
			"logi-rpm-bridge: udp/%d and its fan-out ports (%d-%d) are all taken\n"
			"logi-rpm-bridge: without one of them the texture merge gets no rpm (the engine texture stays flat)\n"
			"logi-rpm-bridge: and the rev lights stay dark. Stop whatever else is reading this telemetry, or\n"
			"logi-rpm-bridge: move us apart with LOGI_RPM_PORT / the daemon's port.relay.\n",
			port, port + 2, port + 1 + FANOUT_PORTS);
		return 1;
	}
	if (!relay.hub)
		fprintf(stderr,
			"logi-rpm-bridge: udp/%d is held by another reader of the same telemetry, so\n"
			"logi-rpm-bridge: reading its fan-out on udp/%d instead. Nothing is lost: the holder\n"
			"logi-rpm-bridge: forwards every datagram, and we take udp/%d back if it exits.\n",
			relay.base, relay.port, relay.base);
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
		ssize_t n = recv(relay.fd, pkt, sizeof(pkt), 0);
		float rpm, max_rpm, first_led = 0.0f;
		struct timespec now;

		/* Paced by the 1 s recv timeout above, so this costs one
		 * bind attempt every couple of seconds while somebody else
		 * holds the relay port, and nothing at all once we hold
		 * it. */
		if (relay_promote(&relay))
			fprintf(stderr,
				"logi-rpm-bridge: took udp/%d back (the reader that held it is gone)\n",
				relay.port);
		if (n < 0) {
			if ((errno == EAGAIN || errno == EWOULDBLOCK) &&
			    t.rev && last_level > 0) {
				write_attr(t.rev, "0", &last_rev_warn);
				last_level = 0;
			}
			continue;
		}
		/* Passed on before it is judged: a datagram this program
		 * cannot use may be exactly what the other reader wants,
		 * and a hub that forwarded only what it understood would be
		 * a filter nobody asked for. */
		relay_forward(&relay, pkt, (size_t)n);
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
	close(relay.fd);
	return 0;
}
