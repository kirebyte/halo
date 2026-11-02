/*
 * Fake RzChromaSDK / RzChromatic for Wine/Proton: receives a game's Razer Chroma effects and
 * forwards them over TCP (127.0.0.1:54236) to the Halo daemon, which paints them on the rings.
 *
 * Signatures and structs come from Razer's public headers (razerofficial/CChromaEditor:
 * RzChromaSDKTypes.h, RzChromaSDK.h, ChromaSDKPluginTypes.h). No color is interpreted here:
 * the DLL only packs (device, type, raw struct bytes) and the daemon decides.
 *
 * Protocol (little endian), one message: u32 length (of what follows) · u8 op · payload
 *   op 1 HELLO: app name in UTF-8
 *   op 2 APPLY: u8 device (Razer's DEVICE_INFO_TYPE: 1 keyboard, 2 mouse, 3 headset,
 *               4 mousepad, 5 keypad, 8 ChromaLink, 255 generic) · u32 effect type · bytes
 * Closing the socket = the game is gone (the daemon drops its layer).
 */
#define WIN32_LEAN_AND_MEAN
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef LONG RZRESULT;
#define RZRESULT_SUCCESS 0L
#define RZRESULT_INVALID_PARAMETER 87L
#define RZRESULT_NOT_FOUND 1168L
#define RZRESULT_NOT_SUPPORTED 50L

enum { DEV_KEYBOARD = 1, DEV_MOUSE = 2, DEV_HEADSET = 3, DEV_MOUSEPAD = 4, DEV_KEYPAD = 5, DEV_CHROMALINK = 8, DEV_GENERIC = 255 };
enum { OP_HELLO = 1, OP_APPLY = 2 };
#define DEFAULT_PORT 54236
#define RECONNECT_MS 2000

typedef struct { int DeviceType; DWORD Connected; } DEVICE_INFO_TYPE;
typedef struct {
    wchar_t Title[256];
    wchar_t Description[1024];
    struct { wchar_t Name[256]; wchar_t Contact[256]; } Author;
    DWORD SupportedDevice;
    DWORD Category;
} APPINFOTYPE;

/* Bytes copied from pParam, per the structs in RzChromaSDKTypes.h (COLORREF = 4).
 * Only the types the daemon knows how to paint; the rest are sent without data (and ignored). */
static size_t param_size(int dev, int type)
{
    switch (dev) {
    case DEV_KEYBOARD:
        switch (type) {
        case 2: return 6 * 22 * 4;                 /* CHROMA_CUSTOM */
        case 4: return 4;                          /* CHROMA_STATIC */
        case 8: return 2 * 6 * 22 * 4;             /* CHROMA_CUSTOM_KEY: Color + Key */
        case 9: return (8 * 24 + 6 * 22) * 4;      /* CHROMA_CUSTOM2 (v2): Color 8x24 + Key 6x22 */
        }
        break;
    case DEV_MOUSE:
        switch (type) {
        case 3: return 30 * 4;                     /* CHROMA_CUSTOM: Color[30] */
        case 6: return 8;                          /* CHROMA_STATIC: RZLED + COLORREF */
        case 8: return 9 * 7 * 4;                  /* CHROMA_CUSTOM2: Color[9][7] */
        }
        break;
    case DEV_HEADSET:
        switch (type) {
        case 1: return 4;                          /* CHROMA_STATIC */
        case 4: return 5 * 4;                      /* CHROMA_CUSTOM: Color[5] */
        }
        break;
    case DEV_MOUSEPAD:
        switch (type) {
        case 2: return 15 * 4;                     /* CHROMA_CUSTOM: Color[15] */
        case 4: return 4;                          /* CHROMA_STATIC */
        case 6: return 20 * 4;                     /* CHROMA_CUSTOM2 (v2): Color[20] */
        }
        break;
    case DEV_KEYPAD:
        switch (type) {
        case 2: return 4 * 5 * 4;                  /* CHROMA_CUSTOM: Color[4][5] */
        case 5: return 4;                          /* CHROMA_STATIC */
        }
        break;
    case DEV_CHROMALINK:
        switch (type) {
        case 1: return 5 * 4;                      /* CHROMA_CUSTOM: Color[5] */
        case 2: return 4;                          /* CHROMA_STATIC */
        }
        break;
    }
    return 0;
}

typedef struct {
    GUID id;
    uint8_t dev;
    uint32_t type;
    uint32_t len;
    uint8_t *data;
} effect_t;

static CRITICAL_SECTION lock;
static SOCKET sock = INVALID_SOCKET;
static ULONGLONG last_attempt;
static int initialized;
static char app_name[256] = "game";
static effect_t *effects;
static size_t n_effects, cap_effects;
static uint32_t next_id = 1;

static unsigned short port(void)
{
    char buf[16];
    DWORD n = GetEnvironmentVariableA("HALO_CHROMA_PORT", buf, sizeof buf);
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
    if (last_attempt && GetTickCount64() - last_attempt < RECONNECT_MS)
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

static void apply(uint8_t dev, uint32_t type, const void *data, uint32_t len)
{
    uint8_t head[5];
    if (!ensure_connected())
        return;
    head[0] = dev;
    memcpy(head + 1, &type, 4);
    send_msg(OP_APPLY, head, 5, data, len);
}

static RZRESULT create(uint8_t dev, int type, void *param, GUID *out)
{
    size_t len = param_size(dev, type);
    uint8_t *copy = NULL;

    if (len && !param)
        return RZRESULT_INVALID_PARAMETER;
    EnterCriticalSection(&lock);
    if (!out) { /* no id: apply immediately (that is how the SDK defines it) */
        apply(dev, (uint32_t)type, param, (uint32_t)len);
        LeaveCriticalSection(&lock);
        return RZRESULT_SUCCESS;
    }
    if (len) {
        copy = malloc(len);
        if (!copy) {
            LeaveCriticalSection(&lock);
            return RZRESULT_INVALID_PARAMETER;
        }
        memcpy(copy, param, len);
    }
    if (n_effects == cap_effects) {
        size_t cap = cap_effects ? cap_effects * 2 : 64;
        effect_t *grown = realloc(effects, cap * sizeof *effects);
        if (!grown) {
            free(copy);
            LeaveCriticalSection(&lock);
            return RZRESULT_INVALID_PARAMETER;
        }
        effects = grown;
        cap_effects = cap;
    }
    memset(out, 0, sizeof *out);
    out->Data1 = next_id++;
    out->Data2 = 0xa0a0; /* "Halo" */
    effects[n_effects].id = *out;
    effects[n_effects].dev = dev;
    effects[n_effects].type = (uint32_t)type;
    effects[n_effects].len = (uint32_t)len;
    effects[n_effects].data = copy;
    n_effects++;
    LeaveCriticalSection(&lock);
    return RZRESULT_SUCCESS;
}

static effect_t *find(GUID id)
{
    for (size_t i = 0; i < n_effects; i++)
        if (IsEqualGUID(&effects[i].id, &id))
            return &effects[i];
    return NULL;
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

EXPORT RZRESULT Init(void)
{
    wchar_t path[MAX_PATH];
    EnterCriticalSection(&lock);
    if (GetModuleFileNameW(NULL, path, MAX_PATH)) {
        wchar_t *base = wcsrchr(path, L'\\');
        WideCharToMultiByte(CP_UTF8, 0, base ? base + 1 : path, -1, app_name, sizeof app_name, NULL, NULL);
    }
    start();
    LeaveCriticalSection(&lock);
    return RZRESULT_SUCCESS;
}

EXPORT RZRESULT InitSDK(APPINFOTYPE *info)
{
    if (!info)
        return Init();
    EnterCriticalSection(&lock);
    if (!WideCharToMultiByte(CP_UTF8, 0, info->Title, -1, app_name, sizeof app_name, NULL, NULL))
        strcpy(app_name, "game");
    start();
    LeaveCriticalSection(&lock);
    return RZRESULT_SUCCESS;
}

EXPORT RZRESULT UnInit(void)
{
    EnterCriticalSection(&lock);
    disconnect();
    for (size_t i = 0; i < n_effects; i++)
        free(effects[i].data);
    n_effects = 0;
    if (initialized) {
        WSACleanup();
        initialized = 0;
    }
    LeaveCriticalSection(&lock);
    return RZRESULT_SUCCESS;
}

EXPORT RZRESULT CreateEffect(GUID device, int type, void *param, GUID *out)
{
    /* Generic, by device model: only CHROMA_STATIC (6) is understood; its struct is
     * { RZSIZE Size; DWORD Param; COLORREF Color; }. Sent as a 4-byte static. */
    (void)device;
    if (type == 6) {
        size_t off = sizeof(size_t) + sizeof(DWORD);
        COLORREF c;
        if (!param)
            return RZRESULT_INVALID_PARAMETER;
        off = (off + 3) & ~(size_t)3;
        memcpy(&c, (uint8_t *)param + off, 4);
        return create(DEV_GENERIC, 4, &c, out) == RZRESULT_SUCCESS ? RZRESULT_SUCCESS : RZRESULT_INVALID_PARAMETER;
    }
    return create(DEV_GENERIC, type == 0 ? 0 : 100 + type, NULL, out);
}

EXPORT RZRESULT CreateKeyboardEffect(int type, void *param, GUID *out) { return create(DEV_KEYBOARD, type, param, out); }
EXPORT RZRESULT CreateMouseEffect(int type, void *param, GUID *out) { return create(DEV_MOUSE, type, param, out); }
EXPORT RZRESULT CreateHeadsetEffect(int type, void *param, GUID *out) { return create(DEV_HEADSET, type, param, out); }
EXPORT RZRESULT CreateMousepadEffect(int type, void *param, GUID *out) { return create(DEV_MOUSEPAD, type, param, out); }
EXPORT RZRESULT CreateKeypadEffect(int type, void *param, GUID *out) { return create(DEV_KEYPAD, type, param, out); }
EXPORT RZRESULT CreateChromaLinkEffect(int type, void *param, GUID *out) { return create(DEV_CHROMALINK, type, param, out); }

EXPORT RZRESULT SetEffect(GUID id)
{
    effect_t *e;
    EnterCriticalSection(&lock);
    e = find(id);
    if (e)
        apply(e->dev, e->type, e->data, e->len);
    LeaveCriticalSection(&lock);
    return e ? RZRESULT_SUCCESS : RZRESULT_NOT_FOUND;
}

EXPORT RZRESULT DeleteEffect(GUID id)
{
    effect_t *e;
    EnterCriticalSection(&lock);
    e = find(id);
    if (e) {
        free(e->data);
        *e = effects[--n_effects];
    }
    LeaveCriticalSection(&lock);
    return e ? RZRESULT_SUCCESS : RZRESULT_NOT_FOUND;
}

EXPORT RZRESULT RegisterEventNotification(HWND hwnd) { (void)hwnd; return RZRESULT_SUCCESS; }
EXPORT RZRESULT UnregisterEventNotification(void) { return RZRESULT_SUCCESS; }
EXPORT RZRESULT SetEventName(const wchar_t *name) { (void)name; return RZRESULT_SUCCESS; }

/* Every device reports as "connected": the daemon decides what to paint. */
EXPORT RZRESULT QueryDevice(GUID device, DEVICE_INFO_TYPE *info)
{
    (void)device;
    if (!info)
        return RZRESULT_INVALID_PARAMETER;
    info->DeviceType = DEV_CHROMALINK;
    info->Connected = 1;
    return RZRESULT_SUCCESS;
}

EXPORT RZRESULT IsConnected(DEVICE_INFO_TYPE *info)
{
    if (!info)
        return RZRESULT_INVALID_PARAMETER;
    info->Connected = 1;
    return RZRESULT_SUCCESS;
}

EXPORT RZRESULT IsActive(BOOL *active)
{
    if (!active)
        return RZRESULT_INVALID_PARAMETER;
    *active = TRUE;
    return RZRESULT_SUCCESS;
}

BOOL WINAPI DllMain(HINSTANCE inst, DWORD reason, LPVOID reserved)
{
    (void)inst;
    (void)reserved;
    if (reason == DLL_PROCESS_ATTACH)
        InitializeCriticalSection(&lock);
    return TRUE;
}
