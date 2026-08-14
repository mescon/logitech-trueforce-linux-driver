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

/* G HUB's dash mapping: dark below first_led, level 1 exactly there,
 * all 10 at the limiter. first_led must sit sanely inside the range or
 * the packet's LED data is treated as absent (-1 = leave LEDs alone). */
static int rev_level(float rpm, float first_led, float max_rpm)
{
	float span;
	int level;

	if (!(first_led > 0.0f) || !(max_rpm > first_led))
		return -1;
	if (rpm < first_led)
		return 0;
	span = max_rpm - first_led;
	level = 1 + (int)(9.0f * (rpm - first_led) / span);
	if (level > 10)
		level = 10;
	return level;
}

static void write_attr(const char *path, const char *val, time_t *last_warn)
{
	FILE *f = fopen(path, "w");

	if (!f) {
		time_t nowt = time(NULL);

		if (nowt - *last_warn >= 30) {
			fprintf(stderr, "logi-rpm-bridge: cannot write %s: %s\n",
				path, strerror(errno));
			*last_warn = nowt;
		}
		return;		/* wheel unplugged; keep listening */
	}
	fputs(val, f);
	fclose(f);
}

int main(void)
{
	char pathbuf[256], revbuf[256];
	const char *path = find_sysfs(pathbuf, sizeof(pathbuf));
	const char *rev_path;
	struct sockaddr_in addr = { 0 };
	unsigned char pkt[64];
	struct timespec last = { 0 };
	time_t last_warn = 0, last_rev_warn = 0;
	int last_level = -1;	/* what the strip currently shows */
	int fd;

	if (!path) {
		fprintf(stderr, "logi-rpm-bridge: no wheel_texture_rpm in sysfs\n");
		return 1;
	}
	rev_path = find_rev_sysfs(path, revbuf, sizeof(revbuf));
	fd = socket(AF_INET, SOCK_DGRAM, 0);
	if (fd < 0) { perror("socket"); return 1; }
	addr.sin_family = AF_INET;
	addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	addr.sin_port = htons(20780);
	{
		int one = 1;
		setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
	}
	{
		/* Bounded recv so a vanished producer darkens the strip:
		 * without telemetry the game is gone and stale lights lie. */
		struct timeval tv = { .tv_sec = 1, .tv_usec = 0 };

		setsockopt(fd, SOL_SOCKET, SO_RCVTIMEO, &tv, sizeof(tv));
	}
	if (bind(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
		perror("bind 20780"); return 1;
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
			    rev_path && last_level > 0) {
				write_attr(rev_path, "0", &last_rev_warn);
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
		if (rev_path) {
			int level = rev_level(rpm, first_led, max_rpm);

			if (level >= 0 && level != last_level) {
				char lv[4];

				snprintf(lv, sizeof(lv), "%d", level);
				write_attr(rev_path, lv, &last_rev_warn);
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
			write_attr(path, val, &last_warn);
		}
	}
	if (rev_path && last_level > 0)
		write_attr(rev_path, "0", &last_rev_warn);
	close(fd);
	return 0;
}
