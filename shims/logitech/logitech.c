/*
 * Fake LogitechLed.dll for Wine/Proton: receives the lighting a game sends through the Logitech
 * LED Illumination SDK and forwards it over TCP (127.0.0.1:54238) to the Halo daemon.
 *
 * Games ship their own LogitechLedEnginesWrapper.dll, which looks up the "real" DLL (the one from
 * Logitech Gaming Software / G HUB) in the registry, in the default value of
 * HKLM\SOFTWARE\Classes\CLSID\{a6519e67-7632-4375-afdf-caa889744403}\ServerBinary, and loads it
 * expecting the same LogiLed* functions. `halo proton install` points that key at this DLL. The
 * export list comes from Artemis' LogitechLed.dll (Artemis.Plugins.Wrappers); signatures from the
 * SDK's public header LogitechLEDLib.h. All are cdecl.
 *
 * No color is interpreted here: the DLL sends the function and its arguments, and the daemon decides.
 * Protocol (little endian), one message: u32 length (of what follows) · u8 op · payload
 *   op 1 HELLO: app name in UTF-8
 *   op 2 CALL:  u8 function · u8 n · i32 args[n] · bytes (only the 21×6 BGRA bitmap)
 * Closing the socket = the game is gone (the daemon drops its layer).
 */
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <wchar.h>

enum { OP_HELLO = 1, OP_CALL = 2 };
/* Functions forwarded to the daemon (crates/halo/src/daemon/bridges/logitech.rs). */
enum {
    F_SET_LIGHTING = 1,  /* r, g, b (percent) */
    F_BITMAP = 2,        /* bytes: 21×6 BGRA */
    F_KEY = 3,           /* code kind (0 scan, 1 HID, 2 quartz, 3 name), code, r, g, b */
    F_FLASH = 4,         /* r, g, b, duration ms, interval ms */
    F_PULSE = 5,         /* r, g, b, duration ms, interval ms */
    F_STOP_EFFECTS = 6,
    F_SAVE = 7,
    F_RESTORE = 8,
    F_TARGET_ZONE = 9,   /* device type, zone, r, g, b */
};
#define BITMAP_SIZE (21 * 6 * 4)
#define DEFAULT_PORT 54238
#define RECONNECT_MS 2000

static CRITICAL_SECTION lock;
static SOCKET sock = INVALID_SOCKET;
static ULONGLONG last_attempt;
static int initialized;
static char app_name[256] = "game";

static unsigned short port(void)
{
    char buf[16];
    DWORD n = GetEnvironmentVariableA("HALO_LOGITECH_PORT", buf, sizeof buf);
    if (n > 0 && n < sizeof buf) {
        long p = strtol(buf, NULL, 10);
        if (p > 0 && p < 65536)
            return (unsigned short)p;
    }
    return DEFAULT_PORT;
}

static int send_all(const void *buf, int len)
{
    const char *p = buf;
    while (len > 0) {
        int n = send(sock, p, len, 0);
        if (n <= 0)
            return 0;
        p += n;
        len -= n;
    }
    return 1;
}

static void disconnect(void)
{
    if (sock != INVALID_SOCKET) {
        closesocket(sock);
        sock = INVALID_SOCKET;
    }
}

static int send_msg(uint8_t op, const void *a, uint32_t alen, const void *b, uint32_t blen)
{
    uint8_t head[5];
    uint32_t len = 1 + alen + blen;
    memcpy(head, &len, 4);
    head[4] = op;
    if (send_all(head, 5) && (!alen || send_all(a, (int)alen)) && (!blen || send_all(b, (int)blen)))
        return 1;
    disconnect();
    return 0;
}

/* Connects if needed; if the daemon is not there, retries every RECONNECT_MS without blocking the game. */
static int ensure_connected(void)
{
    struct sockaddr_in addr;
    SOCKET s;
    int one = 1;

    if (sock != INVALID_SOCKET)
        return 1;
    if (!initialized || (last_attempt && GetTickCount64() - last_attempt < RECONNECT_MS))
        return 0;
    last_attempt = GetTickCount64();
    s = socket(AF_INET, SOCK_STREAM, IPPROTO_TCP);
    if (s == INVALID_SOCKET)
        return 0;
    memset(&addr, 0, sizeof addr);
    addr.sin_family = AF_INET;
    addr.sin_port = htons(port());
    addr.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
    if (connect(s, (struct sockaddr *)&addr, sizeof addr) != 0) {
        closesocket(s);
        return 0;
    }
    setsockopt(s, IPPROTO_TCP, TCP_NODELAY, (const char *)&one, sizeof one);
    sock = s;
    return send_msg(OP_HELLO, app_name, (uint32_t)strlen(app_name), NULL, 0);
}

/* Sends a call with up to 9 integer arguments and, optionally, the bitmap. */
static bool call(uint8_t fn, int n, const int *args, const void *blob, uint32_t blen)
{
    uint8_t buf[2 + 9 * 4];
    EnterCriticalSection(&lock);
    if (ensure_connected()) {
        buf[0] = fn;
        buf[1] = (uint8_t)n;
        memcpy(buf + 2, args, (size_t)n * 4);
        send_msg(OP_CALL, buf, 2 + (uint32_t)n * 4, blob, blen);
    }
    LeaveCriticalSection(&lock);
    return true; /* never fail the game */
}

static void start(void)
{
    WSADATA wsa;
    if (initialized)
        return;
    WSAStartup(MAKEWORD(2, 2), &wsa);
    initialized = 1;
    last_attempt = 0;
    ensure_connected();
}

#define EXPORT __declspec(dllexport)

/* Without an explicit name, the session is named after the game's .exe. */
EXPORT bool LogiLedInitWithName(const char name[])
{
    wchar_t path[MAX_PATH];
    EnterCriticalSection(&lock);
    if (!initialized) {
        if (name && *name) {
            strncpy(app_name, name, sizeof app_name - 1);
            app_name[sizeof app_name - 1] = 0;
        } else if (GetModuleFileNameW(NULL, path, MAX_PATH)) {
            wchar_t *base = wcsrchr(path, L'\\');
            WideCharToMultiByte(CP_UTF8, 0, base ? base + 1 : path, -1, app_name, sizeof app_name, NULL, NULL);
        }
    }
    start();
    LeaveCriticalSection(&lock);
    return true;
}

EXPORT bool LogiLedInit(void) { return LogiLedInitWithName(NULL); }

EXPORT bool LogiLedGetSdkVersion(int *major, int *minor, int *build)
{
    if (major) *major = 9;
    if (minor) *minor = 0;
    if (build) *build = 0;
    return true;
}

/* User-configurable options in LGS: there is no panel here, so the game keeps its defaults. */
EXPORT bool LogiGetConfigOptionNumber(const wchar_t *path, double *def, int id) { (void)path; (void)def; (void)id; return true; }
EXPORT bool LogiGetConfigOptionBool(const wchar_t *path, bool *def, int id) { (void)path; (void)def; (void)id; return true; }
EXPORT bool LogiGetConfigOptionColor(const wchar_t *path, int *r, int *g, int *b, int id) { (void)path; (void)r; (void)g; (void)b; (void)id; return true; }
EXPORT bool LogiGetConfigOptionRect(const wchar_t *path, int *x, int *y, int *w, int *h, int id) { (void)path; (void)x; (void)y; (void)w; (void)h; (void)id; return true; }
EXPORT bool LogiGetConfigOptionKeyInput(const wchar_t *path, wchar_t *def, int size, int id) { (void)path; (void)def; (void)size; (void)id; return true; }
EXPORT bool LogiGetConfigOptionSelect(const wchar_t *path, wchar_t *def, int *size, const wchar_t *values, int bufsize, int id) { (void)path; (void)def; (void)size; (void)values; (void)bufsize; (void)id; return true; }
EXPORT bool LogiGetConfigOptionRange(const wchar_t *path, int *def, int min, int max, int id) { (void)path; (void)def; (void)min; (void)max; (void)id; return true; }
EXPORT bool LogiSetConfigOptionLabel(const wchar_t *path, wchar_t *label, int id) { (void)path; (void)label; (void)id; return true; }

/* Every device type is accepted: the daemon decides. */
EXPORT bool LogiLedSetTargetDevice(int target) { (void)target; return true; }

EXPORT bool LogiLedSaveCurrentLighting(void) { return call(F_SAVE, 0, NULL, NULL, 0); }
EXPORT bool LogiLedRestoreLighting(void) { return call(F_RESTORE, 0, NULL, NULL, 0); }
EXPORT bool LogiLedStopEffects(void) { return call(F_STOP_EFFECTS, 0, NULL, NULL, 0); }

EXPORT bool LogiLedSetLighting(int r, int g, int b)
{
    int a[] = { r, g, b };
    return call(F_SET_LIGHTING, 3, a, NULL, 0);
}

EXPORT bool LogiLedFlashLighting(int r, int g, int b, int duration, int interval)
{
    int a[] = { r, g, b, duration, interval };
    return call(F_FLASH, 5, a, NULL, 0);
}

EXPORT bool LogiLedPulseLighting(int r, int g, int b, int duration, int interval)
{
    int a[] = { r, g, b, duration, interval };
    return call(F_PULSE, 5, a, NULL, 0);
}

EXPORT bool LogiLedSetLightingFromBitmap(unsigned char bitmap[])
{
    if (!bitmap)
        return false;
    return call(F_BITMAP, 0, NULL, bitmap, BITMAP_SIZE);
}

static bool key(int kind, int code, int r, int g, int b)
{
    int a[] = { kind, code, r, g, b };
    return call(F_KEY, 5, a, NULL, 0);
}

EXPORT bool LogiLedSetLightingForKeyWithScanCode(int code, int r, int g, int b) { return key(0, code, r, g, b); }
EXPORT bool LogiLedSetLightingForKeyWithHidCode(int code, int r, int g, int b) { return key(1, code, r, g, b); }
EXPORT bool LogiLedSetLightingForKeyWithQuartzCode(int code, int r, int g, int b) { return key(2, code, r, g, b); }
EXPORT bool LogiLedSetLightingForKeyWithKeyName(int name, int r, int g, int b) { return key(3, name, r, g, b); }

EXPORT bool LogiLedSetLightingForTargetZone(int device, int zone, int r, int g, int b)
{
    int a[] = { device, zone, r, g, b };
    return call(F_TARGET_ZONE, 5, a, NULL, 0);
}

/* Per-key effects: without a position for each key on the rings there is nothing to paint. */
EXPORT bool LogiLedSaveLightingForKey(int name) { (void)name; return true; }
EXPORT bool LogiLedRestoreLightingForKey(int name) { (void)name; return true; }
EXPORT bool LogiLedExcludeKeysFromBitmap(int *keys, int count) { (void)keys; (void)count; return true; }
EXPORT bool LogiLedFlashSingleKey(int name, int r, int g, int b, int duration, int interval) { (void)name; (void)r; (void)g; (void)b; (void)duration; (void)interval; return true; }
EXPORT bool LogiLedPulseSingleKey(int name, int r0, int g0, int b0, int r1, int g1, int b1, int duration, bool infinite) { (void)name; (void)r0; (void)g0; (void)b0; (void)r1; (void)g1; (void)b1; (void)duration; (void)infinite; return true; }
EXPORT bool LogiLedStopEffectsOnKey(int name) { (void)name; return true; }

EXPORT void LogiLedShutdown(void)
{
    EnterCriticalSection(&lock);
    disconnect();
    if (initialized) {
        WSACleanup();
        initialized = 0;
    }
    LeaveCriticalSection(&lock);
}

/* LGS registers it as a COM server; here the registry entry is written by `halo proton install`. */
EXPORT HRESULT DllRegisterServer(void) { return S_OK; }
EXPORT HRESULT DllUnregisterServer(void) { return S_OK; }

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved)
{
    (void)inst;
    (void)reserved;
    if (reason == DLL_PROCESS_ATTACH)
        InitializeCriticalSection(&lock);
    return TRUE;
}
