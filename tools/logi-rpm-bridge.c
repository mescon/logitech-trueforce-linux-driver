/* logi-rpm-bridge: forward the game telemetry relay's RPM to the driver.
 *
 * Listens for LTFR datagrams on 127.0.0.1:20780 (the dinput8 escape
 * proxy's telemetry relay) and writes "rpm max_rpm" to the wheel's
 * wheel_texture_rpm sysfs attribute, which feeds the native texture
 * merge. Exits 0 on SIGTERM/SIGINT. Build:
 *   cc -O2 -Wall -o logi-rpm-bridge logi-rpm-bridge.c
 */
#include <arpa/inet.h>
#include <glob.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
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
	if (!glob("/sys/bus/hid/devices/*046D:C27*/wheel_texture_rpm",
		  0, NULL, &g) && g.gl_pathc) {
		snprintf(buf, n, "%s", g.gl_pathv[0]);
		globfree(&g);
		return buf;
	}
	globfree(&g);
	return NULL;
}

int main(void)
{
	char pathbuf[256];
	const char *path = find_sysfs(pathbuf, sizeof(pathbuf));
	struct sockaddr_in addr = { 0 };
	unsigned char pkt[64];
	struct timespec last = { 0 };
	int fd;

	if (!path) {
		fprintf(stderr, "logi-rpm-bridge: no wheel_texture_rpm in sysfs\n");
		return 1;
	}
	fd = socket(AF_INET, SOCK_DGRAM, 0);
	if (fd < 0) { perror("socket"); return 1; }
	addr.sin_family = AF_INET;
	addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
	addr.sin_port = htons(20780);
	{
		int one = 1;
		setsockopt(fd, SOL_SOCKET, SO_REUSEADDR, &one, sizeof(one));
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
		float rpm, max_rpm;
		struct timespec now;
		FILE *f;

		if (n < 28 || memcmp(pkt, "LTFR", 4) || pkt[4] != 2)
			continue;
		memcpy(&rpm, pkt + 14, 4);
		memcpy(&max_rpm, pkt + 18, 4);
		if (!(rpm >= 0.0f && rpm < 30000.0f))
			continue;
		/* max_rpm shares the kernel store's bound (wheel_texture_rpm_store
		 * rejects > 30000), but a bad max_rpm should not throw away a good
		 * rpm sample: clamp instead of dropping the packet. */
		if (max_rpm < 0.0f)
			max_rpm = 0.0f;
		else if (max_rpm >= 30000.0f)
			max_rpm = 29999.0f;
		clock_gettime(CLOCK_MONOTONIC, &now);
		if (now.tv_sec == last.tv_sec &&
		    now.tv_nsec - last.tv_nsec < 10 * 1000 * 1000)
			continue;
		last = now;
		f = fopen(path, "w");
		if (!f)
			continue;	/* wheel unplugged; keep listening */
		fprintf(f, "%.0f %.0f", rpm, max_rpm);
		fclose(f);
	}
	close(fd);
	return 0;
}
