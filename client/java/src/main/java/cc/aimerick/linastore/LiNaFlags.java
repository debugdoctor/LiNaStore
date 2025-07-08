package cc.aimerick.linastore;

public enum LiNaFlags {
    DELETE(0xC0),
    WRITE(0x80),
    READ(0x40),
    COVER(0x02),
    COMPRESS(0x01),
    NONE(0x00);

    private final int value;

    LiNaFlags(int value) {
        this.value = value;
    }

    public int getValue() {
        return value;
    }
}