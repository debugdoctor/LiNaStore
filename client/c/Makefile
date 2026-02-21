CC ?= gcc
CFLAGS ?= -O2 -Wall -Wextra -fPIC
INCLUDES := -Isrc
LIBS := -lssl -lcrypto

SRC := src/linaclient.c src/crc32.c
OBJ := $(SRC:.c=.o)

.PHONY: all clean install

all: liblina_client.a liblina_client.so

liblina_client.a: $(OBJ)
	$(AR) rcs $@ $^

liblina_client.so: $(OBJ)
	$(CC) -shared -o $@ $^ $(LIBS)

src/%.o: src/%.c
	$(CC) $(CFLAGS) $(INCLUDES) -c $< -o $@

install: all
	mkdir -p $(DESTDIR)/usr/local/lib $(DESTDIR)/usr/local/include
	cp liblina_client.a liblina_client.so $(DESTDIR)/usr/local/lib
	cp src/linaclient.h src/crc32.h $(DESTDIR)/usr/local/include

clean:
	rm -f $(OBJ) liblina_client.a liblina_client.so
