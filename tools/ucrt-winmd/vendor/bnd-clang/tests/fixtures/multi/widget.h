#pragma once
#include "types.h"

// Struct with cross-partition type reference (Color from types.h)
typedef struct {
    const char* name;
    int values[4];
    Color color;
} Widget;

// Functions referencing types from types.h
int create_widget(const char* name, Rect bounds, Widget* out);
void destroy_widget(Widget* w);
int widget_count(void);
