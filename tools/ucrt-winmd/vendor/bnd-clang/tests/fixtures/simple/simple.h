#pragma once

#include <stddef.h>

// Enum
typedef enum {
    COLOR_RED   = 0,
    COLOR_GREEN = 1,
    COLOR_BLUE  = 2,
} Color;

// Struct with basic fields
typedef struct {
    int x;
    int y;
    unsigned int width;
    unsigned int height;
} Rect;

// Struct with pointer and array
typedef struct {
    const char* name;
    int values[4];
    Color color;
} Widget;

// Function pointer (delegate)
typedef int (*CompareFunc)(const void* a, const void* b);
typedef int FunctionType(void* context);
typedef struct {
    FunctionType* callback;
} FunctionTable;
FunctionType* get_function_type(int selector);
typedef void (*SignalHandler)(int);
typedef SignalHandler SignalHandlerAlias;
#define TEST_SIG_DEFAULT ((SignalHandler)0)
#define TEST_SIG_ALIAS ((SignalHandlerAlias)0)

// Union
typedef union {
    int i;
    float f;
    unsigned char bytes[4];
} Value;

// Struct with anonymous nested union (like in6_addr)
typedef struct {
    union {
        unsigned char  bytes[16];
        unsigned short words[8];
        unsigned int   dwords[4];
    } addr;
    unsigned int scope_id;
} NetAddr;

// Macro-expanded named nested union. Clang reports no source file for the
// field's union declaration even though the nested definition is emitted.
typedef struct MacroNestedUnion MacroNestedUnion;
#define DEFINE_NESTED_UNION(name) \
    struct name { \
        union name##_dummy { \
            void *d1; \
            unsigned long d2; \
            int d3; \
        } dummy; \
    };
DEFINE_NESTED_UNION(MacroNestedUnion)

// Functions
int create_widget(const char* name, Rect bounds, Widget* out);
void destroy_widget(Widget* w);
int widget_count(void);
int consume_va_list(__builtin_va_list args);
int redirected_scan(const char* input);
int redirected_scan(const char* input) __asm__("actual_scan");
typedef __builtin_va_list test_va_list;
typedef struct {
    test_va_list args;
    int tail;
} SavedVaList;
typedef __builtin_va_list AlignedArgs __attribute__((aligned(16)));
typedef struct {
    char first;
    AlignedArgs args;
    int tail;
} StoredAlignedArgs;
void consume_stored_aligned_args(StoredAlignedArgs* value);

// #define constants
#define MAX_WIDGETS 256
#define DEFAULT_WIDTH 800
#define DEFAULT_HEIGHT 600
#define FIRST_FLAG 0x1
#define SECOND_FLAG 0x2
#define COMBINED_FLAGS (FIRST_FLAG | SECOND_FLAG)
#define HIGH_BIT (1U << 31)
#define BUFFER_BYTES ((size_t)(FIRST_FLAG << 4))
#define _IOFBF 0
#define _IOLBF 1
#define _IONBF 2

// Conditional constant controlled by global clang_args in simple.toml.
// Tests that top-level clang_args = ["-DCUSTOM_DEPTH=42"] is applied.
#ifdef CUSTOM_DEPTH
#define MAX_DEPTH 42
#endif

// Typedef that shadows a Rust primitive — must be suppressed to avoid
// `pub type bool = bool;` (recursive type alias).
#include <stdbool.h>
#undef bool
typedef _Bool bool;

// Function that uses the bool typedef to verify it still works after
// the typedef is suppressed.
bool widget_is_visible(const Widget* w);

// __int128 typedefs — must be silently skipped (no WinMD 128-bit type).
// These must not cause a build error or produce `pub type __s128 = isize;`.
// Typedef chains through __int128 must also be skipped recursively.
typedef __int128 __s128;
typedef unsigned __int128 __u128;
typedef __s128 s128;
typedef __u128 u128;
typedef __float128 f128;
typedef f128 chained_f128;
typedef _Complex double complex64;

// C11 anonymous union member (no field name) — the union's fields
// should be accessible and the struct should have correct size/offsets.
typedef struct {
    int before;
    union { int x; float y; };
    int after;
} HasAnonUnion;

// Struct with alignment attribute — trailing padding must be preserved.
// sizeof(CacheAligned) == 64 (8 bytes of fields padded to 64).
struct CacheAligned {
    int x;
    int y;
} __attribute__((aligned(64)));

// Enum used in a bitfield — tests whether sonar discovers the enum
// and whether the struct layout is correct despite bitfield packing.
enum BitfieldKind {
    BF_KIND_NONE  = 0,
    BF_KIND_FLAG  = 1,
    BF_KIND_VALUE = 2,
};

struct WithBitfield {
    const char *name;
    enum BitfieldKind kind:8;
    unsigned int flags:24;
    int data;
};

// Anonymous struct used as 2D array element — tests multi-dimensional peeling.
// sizeof == 4 * 8 * 4 (tc_rxq) + 4 (count) == 132
struct WithAnon2DArrayField {
    struct {
        unsigned short base;
        unsigned short nb_queue;
    } tc_rxq[4][8];
    int count;
};
// the anonymous type and wraps the field in CType::Array correctly.
// sizeof == 4 * (2+2 padding + 4) + 4 + 4 padding == 40
struct WithAnonArrayField {
    struct {
        unsigned short id;
        unsigned int   mask;
    } entries[4];
    int count;
};

// Struct embedding a cache-aligned struct — tests inter-field padding.
// The embedded `aligned_member` field must start at offset 64 (not at
// offset 16 which is where natural alignment would place it).
// This reproduces the ____cacheline_aligned_in_smp kernel bug.
struct AlignedInner {
    long a;
    long b;
} __attribute__((aligned(64)));

struct EmbeddingAligned {
    long before_a;
    long before_b;
    struct AlignedInner aligned_member;
    int after;
};

typedef struct {
    char bytes[104];
} UnrepresentableAligned __attribute__((aligned(16)));

void consume_unrepresentable(UnrepresentableAligned* value);
typedef void (*UnrepresentableCallback)(UnrepresentableAligned* value);
typedef UnrepresentableCallback UnrepresentableCallbackAlias;
typedef struct {
    UnrepresentableAligned* value;
} UnrepresentableHolder;
typedef union {
    char bytes[104];
} UnrepresentableUnion __attribute__((aligned(16)));
void consume_unrepresentable_callback(UnrepresentableCallbackAlias callback);
void consume_unrepresentable_union(UnrepresentableUnion* value);
