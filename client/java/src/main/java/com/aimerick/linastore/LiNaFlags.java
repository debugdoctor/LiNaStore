package com.aimerick.linastore;

public enum LiNaFlags {
    READ(0x40),    // 01000000 - bits 7:5 = 010 (read operation)
    AUTH(0x60),    // 01100000 - bits 7:5 = 011 (authentication)
    WRITE(0x80),   // 10000000 - bits 7:5 = 100 (write operation)
    DELETE(0xC0),  // 11000000 - bits 7:5 = 110 (delete operation)
    COVER(0x02),   // 00000010 - bit 1 (overwrite flag)
    COMPRESS(0x01),// 00000001 - bit 0 (compression flag)
    NONE(0x00);    // 00000000 (no operation)

    private final int value;

    LiNaFlags(int value) {
        this.value = value;
    }

    public int getValue() {
        return value;
    }
}