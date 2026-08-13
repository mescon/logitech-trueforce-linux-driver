/* Userspace tests for the pure texture-merge logic. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include "hidpp_dd_texture_merge.h"

static int failures;
#define CHECK(cond, ...) do { \
	if (!(cond)) { failures++; printf("FAIL %s:%d: ", __FILE__, __LINE__); \
		       printf(__VA_ARGS__); printf("\n"); } \
} while (0)

static void test_lut_sanity(void)
{
	CHECK(hidpp_dd_texmerge_sine_lut[0] == 0, "sin(0) != 0");
	CHECK(hidpp_dd_texmerge_sine_lut[256] == 32767, "sin(pi/2) != max");
	CHECK(hidpp_dd_texmerge_sine_lut[512] == 0, "sin(pi) != 0");
	CHECK(hidpp_dd_texmerge_sine_lut[768] == -32767, "sin(3pi/2) != min");
}

int main(void)
{
	test_lut_sanity();
	printf(failures ? "%d FAILURES\n" : "all tests pass\n", failures);
	return failures ? 1 : 0;
}
