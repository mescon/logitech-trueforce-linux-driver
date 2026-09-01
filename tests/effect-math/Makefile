CFLAGS := -Wall -Wextra -Werror -O2 -I../../mainline
test_effect_math: test_effect_math.c ../../mainline/hidpp_dd_effect_math.h
	$(CC) $(CFLAGS) -o $@ test_effect_math.c
run: test_effect_math
	./test_effect_math
clean:
	rm -f test_effect_math
.PHONY: run clean
