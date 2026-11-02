# Fake Chroma DLL for Wine/Proton (64- and 32-bit). Needs mingw-w64-gcc.
# Built under the four names games look for: RzChromaSDK(64).dll and RzChromatic(64).dll.
CFLAGS = -O2 -Wall -Wextra -std=c11 -static-libgcc
LIBS = -lws2_32
OUT = build

all: $(OUT)/x64/RzChromaSDK64.dll $(OUT)/x64/RzChromatic64.dll $(OUT)/x86/RzChromaSDK.dll $(OUT)/x86/RzChromatic.dll

$(OUT)/x64/RzChromaSDK64.dll: chroma.c chroma.def version.rc
	@mkdir -p $(@D)
	x86_64-w64-mingw32-windres version.rc -O coff -o $(OUT)/x64/version.o
	x86_64-w64-mingw32-gcc $(CFLAGS) -shared -o $@ chroma.c chroma.def $(OUT)/x64/version.o $(LIBS)

$(OUT)/x86/RzChromaSDK.dll: chroma.c chroma.def version.rc
	@mkdir -p $(@D)
	i686-w64-mingw32-windres version.rc -O coff -o $(OUT)/x86/version.o
	i686-w64-mingw32-gcc $(CFLAGS) -shared -Wl,--kill-at -o $@ chroma.c chroma.def $(OUT)/x86/version.o $(LIBS)

$(OUT)/x64/RzChromatic64.dll: $(OUT)/x64/RzChromaSDK64.dll
	cp $< $@

$(OUT)/x86/RzChromatic.dll: $(OUT)/x86/RzChromaSDK.dll
	cp $< $@

test: all $(OUT)/x64/test.exe $(OUT)/x86/test.exe

$(OUT)/x64/test.exe: test.c
	x86_64-w64-mingw32-gcc -O2 -Wall -o $@ test.c -lversion

$(OUT)/x86/test.exe: test.c
	i686-w64-mingw32-gcc -O2 -Wall -o $@ test.c -lversion

clean:
	rm -rf $(OUT)

.PHONY: all test clean
