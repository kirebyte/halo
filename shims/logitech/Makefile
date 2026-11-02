# Fake LogitechLed.dll for Wine/Proton (64- and 32-bit). Needs mingw-w64-gcc.
CFLAGS = -O2 -Wall -Wextra -std=c11 -static-libgcc
LIBS = -lws2_32
OUT = build

all: $(OUT)/x64/LogitechLed.dll $(OUT)/x86/LogitechLed.dll

$(OUT)/x64/LogitechLed.dll: logitech.c logitech.def version.rc
	@mkdir -p $(@D)
	x86_64-w64-mingw32-windres version.rc -O coff -o $(OUT)/x64/version.o
	x86_64-w64-mingw32-gcc $(CFLAGS) -shared -o $@ logitech.c logitech.def $(OUT)/x64/version.o $(LIBS)

$(OUT)/x86/LogitechLed.dll: logitech.c logitech.def version.rc
	@mkdir -p $(@D)
	i686-w64-mingw32-windres version.rc -O coff -o $(OUT)/x86/version.o
	i686-w64-mingw32-gcc $(CFLAGS) -shared -o $@ logitech.c logitech.def $(OUT)/x86/version.o $(LIBS)

test: all $(OUT)/x64/test.exe $(OUT)/x86/test.exe

$(OUT)/x64/test.exe: test.c
	x86_64-w64-mingw32-gcc -O2 -Wall -o $@ test.c -ladvapi32

$(OUT)/x86/test.exe: test.c
	i686-w64-mingw32-gcc -O2 -Wall -o $@ test.c -ladvapi32

clean:
	rm -rf $(OUT)

.PHONY: all test clean
