CXX ?= g++
CXXFLAGS ?= -O2 -Wall -Wextra -fPIC -std=c++17
INCLUDES := -Isrc
LIBS := -lssl -lcrypto

SRC := src/linaclient.cpp src/crc32.cpp src/token_management.cpp
OBJ := $(SRC:.cpp=.o)

.PHONY: all clean install

all: liblina_client_cpp.a liblina_client_cpp.so

liblina_client_cpp.a: $(OBJ)
	$(AR) rcs $@ $^

liblina_client_cpp.so: $(OBJ)
	$(CXX) -shared -o $@ $^ $(LIBS)

src/%.o: src/%.cpp
	$(CXX) $(CXXFLAGS) $(INCLUDES) -c $< -o $@

install: all
	mkdir -p $(DESTDIR)/usr/local/lib $(DESTDIR)/usr/local/include
	cp liblina_client_cpp.a liblina_client_cpp.so $(DESTDIR)/usr/local/lib
	cp src/linaclient.h src/crc32.h $(DESTDIR)/usr/local/include

clean:
	rm -f $(OBJ) liblina_client_cpp.a liblina_client_cpp.so
