#pragma once

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

// Function pointer (delegate)
typedef int (*CompareFunc)(const void* a, const void* b);

// #define constants
#define MAX_WIDGETS 256
#define DEFAULT_WIDTH 800
#define DEFAULT_HEIGHT 600
