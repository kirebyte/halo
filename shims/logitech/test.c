/* Test of the DLL under Wine: does what LogitechLedEnginesWrapper.dll (the one games ship)
 * does: reads the path from the registry, loads the DLL, resolves every function and sends lighting.
 *   wine test.exe [ms between steps] */
#include <windows.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>

#define KEY L"SOFTWARE\\Classes\\CLSID\\{a6519e67-7632-4375-afdf-caa889744403}\\ServerBinary"

static const char *names[] = {
    "LogiLedInit", "LogiLedInitWithName", "LogiLedGetSdkVersion", "LogiGetConfigOptionNumber",
    "LogiGetConfigOptionBool", "LogiGetConfigOptionColor", "LogiGetConfigOptionRect",
    "LogiGetConfigOptionKeyInput", "LogiGetConfigOptionSelect", "LogiGetConfigOptionRange",
    "LogiSetConfigOptionLabel", "LogiLedSetTargetDevice", "LogiLedSaveCurrentLighting",
    "LogiLedRestoreLighting", "LogiLedStopEffects", "LogiLedSetLighting", "LogiLedFlashLighting",
    "LogiLedPulseLighting", "LogiLedSetLightingFromBitmap", "LogiLedSetLightingForKeyWithScanCode",
    "LogiLedSetLightingForKeyWithHidCode", "LogiLedSetLightingForKeyWithQuartzCode",
    "LogiLedSetLightingForKeyWithKeyName", "LogiLedSetLightingForTargetZone",
    "LogiLedSaveLightingForKey", "LogiLedRestoreLightingForKey", "LogiLedExcludeKeysFromBitmap",
    "LogiLedFlashSingleKey", "LogiLedPulseSingleKey", "LogiLedStopEffectsOnKey", "LogiLedShutdown",
};

typedef bool (*init_name_t)(const char *);
typedef bool (*rgb_t)(int, int, int);
typedef bool (*effect_t)(int, int, int, int, int);
typedef bool (*bitmap_t)(unsigned char *);
typedef bool (*int_t)(int);
typedef void (*void_t)(void);

int main(int argc, char **argv)
{
    int wait = argc > 1 ? atoi(argv[1]) : 2500;
    wchar_t path[MAX_PATH];
    DWORD size = sizeof path;
    HMODULE lib;
    unsigned char bitmap[21 * 6 * 4] = { 0 };

    if (RegGetValueW(HKEY_LOCAL_MACHINE, KEY, NULL, RRF_RT_REG_SZ, NULL, path, &size) != ERROR_SUCCESS) {
        printf("FAIL: no ServerBinary in the registry\n");
        return 1;
    }
    printf("registry -> %ls\n", path);
    lib = LoadLibraryW(path);
    if (!lib) {
        printf("FAIL: does not load (%lu)\n", GetLastError());
        return 1;
    }
    for (size_t i = 0; i < sizeof names / sizeof *names; i++)
        if (!GetProcAddress(lib, names[i])) {
            printf("FAIL: missing %s\n", names[i]);
            return 1;
        }
    printf("all %u functions present\n", (unsigned)(sizeof names / sizeof *names));

    ((init_name_t)GetProcAddress(lib, "LogiLedInitWithName"))("Logitech test");
    ((int_t)GetProcAddress(lib, "LogiLedSetTargetDevice"))(7);
    printf("1. solid magenta\n");
    ((rgb_t)GetProcAddress(lib, "LogiLedSetLighting"))(100, 0, 100);
    Sleep(wait);
    printf("2. bitmap: left green, right blue\n");
    for (int r = 0; r < 6; r++)
        for (int c = 0; c < 21; c++) {
            unsigned char *px = bitmap + (r * 21 + c) * 4; /* BGRA */
            px[c < 10 ? 1 : 0] = 255;
            px[3] = 255;
        }
    ((bitmap_t)GetProcAddress(lib, "LogiLedSetLightingFromBitmap"))(bitmap);
    Sleep(wait);
    printf("3. cyan flash for 3 s (then back to the bitmap)\n");
    ((effect_t)GetProcAddress(lib, "LogiLedFlashLighting"))(0, 100, 100, 3000, 500);
    Sleep(3500);
    printf("4. orange pulse for 4 s\n");
    ((effect_t)GetProcAddress(lib, "LogiLedPulseLighting"))(100, 50, 0, 4000, 2000);
    Sleep(4500);
    printf("5. bye (Shutdown)\n");
    ((void_t)GetProcAddress(lib, "LogiLedShutdown"))();
    return 0;
}
