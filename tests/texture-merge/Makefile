CFLAGS := -Wall -Wextra -Werror -O2 -I../../mainline
test_texmerge: test_texmerge.c ../../mainline/hidpp_dd_texture_merge.h
	$(CC) $(CFLAGS) -o $@ test_texmerge.c -lm
run: test_texmerge
	./test_texmerge
clean:
	rm -f test_texmerge
.PHONY: run clean
