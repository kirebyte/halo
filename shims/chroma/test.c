/* Test of the DLL under Wine: does what Razer's sample library does
 * (checks FileVersion >= 1.0.0.36, resolves every function) and sends effects.
 *   wine test.exe [ms between steps] */
#include <windows.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#ifdef _WIN64
#define DLL L"RzChromatic64.dll"
#else
#define DLL L"RzChromatic.dll"
#endif

typedef LONG (*init_sdk_t)(void *);
typedef LONG (*void_t)(void);
typedef LONG (*create_t)(int, void *, GUID *);
typedef LONG (*set_t)(GUID);

static const char *names[] = {
    "Init", "InitSDK", "UnInit", "CreateEffect", "CreateKeyboardEffect", "CreateMouseEffect",
    "CreateHeadsetEffect", "CreateMousepadEffect", "CreateKeypadEffect", "CreateChromaLinkEffect",
    "SetEffect", "DeleteEffect", "RegisterEventNotification", "UnregisterEventNotification",
    "SetEventName", "QueryDevice", "IsConnected", "IsActive",
};

#define RGB_(r, g, b) ((DWORD)(r) | ((DWORD)(g) << 8) | ((DWORD)(b) << 16))

int main(int argc, char **argv)
{
    int wait = argc > 1 ? atoi(argv[1]) : 1500;
    DWORD h, size = GetFileVersionInfoSizeW(DLL, &h);
    void *ver;
    VS_FIXEDFILEINFO *fi;
    UINT len;
    HMODULE lib;

    if (!size) {
        printf("FAIL: cannot find %ls or it has no VERSIONINFO\n", DLL);
        return 1;
    }
    ver = malloc(size);
    GetFileVersionInfoW(DLL, 0, size, ver);
    VerQueryValueW(ver, L"\\", (void **)&fi, &len);
    printf("version %u.%u.%u.%u (Razer requires >= 1.0.0.36)\n", HIWORD(fi->dwFileVersionMS), LOWORD(fi->dwFileVersionMS),
           HIWORD(fi->dwFileVersionLS), LOWORD(fi->dwFileVersionLS));

    lib = LoadLibraryW(DLL);
    if (!lib) {
        printf("FAIL: LoadLibrary\n");
        return 1;
    }
    for (size_t i = 0; i < sizeof names / sizeof *names; i++)
        if (!GetProcAddress(lib, names[i])) {
            printf("FAIL: missing %s\n", names[i]);
            return 1;
        }
    printf("all %zu functions present\n", sizeof names / sizeof *names);

    struct { wchar_t Title[256]; wchar_t Description[1024]; wchar_t Name[256]; wchar_t Contact[256]; DWORD Dev, Cat; } info;
    memset(&info, 0, sizeof info);
    wcscpy(info.Title, L"Halo test");
    printf("InitSDK -> %ld\n", ((init_sdk_t)GetProcAddress(lib, "InitSDK"))(&info));

    create_t link = (create_t)GetProcAddress(lib, "CreateChromaLinkEffect");
    create_t kb = (create_t)GetProcAddress(lib, "CreateKeyboardEffect");
    set_t set = (set_t)GetProcAddress(lib, "SetEffect");
    set_t del = (set_t)GetProcAddress(lib, "DeleteEffect");

    /* 1. ChromaLink CUSTOM: background + 4 colors (cyan, magenta, orange, white) */
    DWORD cl[5] = {RGB_(40, 40, 40), RGB_(0, 255, 255), RGB_(255, 0, 255), RGB_(255, 96, 0), RGB_(255, 255, 255)};
    GUID id;
    printf("ChromaLink custom -> %ld", link(1, cl, &id));
    printf(", SetEffect -> %ld\n", set(id));
    Sleep(wait);

    /* 2. Keyboard CUSTOM: one purple column on the left, one green column on the right, black elsewhere */
    static DWORD grid[6][22];
    memset(grid, 0, sizeof grid);
    for (int r = 0; r < 6; r++) {
        grid[r][2] = RGB_(128, 0, 255);
        grid[r][18] = RGB_(0, 255, 0);
    }
    printf("ChromaLink NONE -> %ld\n", link(0, NULL, NULL));
    printf("Keyboard custom, immediate -> %ld\n", kb(2, grid, NULL));
    Sleep(wait);

    /* 3. Static orange via keyboard */
    DWORD orange = RGB_(255, 96, 0);
    printf("Keyboard static -> %ld\n", kb(4, &orange, NULL));
    Sleep(wait);

    LONG first = del(id);
    printf("DeleteEffect -> %ld, again -> %ld (1168 = not found)\n", first, del(id));
    printf("UnInit -> %ld\n", ((void_t)GetProcAddress(lib, "UnInit"))());
    return 0;
}
