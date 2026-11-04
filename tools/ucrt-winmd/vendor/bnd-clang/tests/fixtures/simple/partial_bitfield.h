#pragma once

struct PartialBitfield {
    unsigned int bits : 24;
    unsigned char next;
};
