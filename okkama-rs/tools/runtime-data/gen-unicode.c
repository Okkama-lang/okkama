/*
 * Таблицы Unicode для natives runtime/unicode_character из ICU4C 78
 * (Unicode 17.0.0, той же версии, что сегментация графем unicode-segmentation,
 * #1019): имена, категории, combining class, числовые значения и свойства
 * ширины. Формат тот же, что давал прежний gen-unicode.pl (Unicode::UCD
 * Perl 5.42, Unicode 16.0.0); на символах 16.0.0 вывод совпадает с ним, кроме
 * правок самого Unicode 17.0.0. Вывод — текст на stdout, сжать gzip -9n:
 *   V <major>.<minor>.<update>   версия Unicode (тест рантайма сверяет её
 *                                с версией сегментации графем)
 *   R <prop> <from> <to>         диапазон с булевым свойством
 *   G <cat> <from> <to>          общая категория
 *   C <ccc> <from> <to>          combining class (кроме 0)
 *   N <cp> <value>               числовое значение (целое или n/d)
 *   M <cp> <name>                имя символа
 *   A <prefix> <from> <to>       алгоритмические имена <prefix>-<HEX>
 *   H <from> <to>                слоги хангыль (имя по алгоритму Unicode)
 *
 * Сборка и запуск (Homebrew):
 *   P=$(brew --prefix icu4c@78)
 *   cc -I$P/include -L$P/lib -licuuc -Wl,-rpath,$P/lib gen-unicode.c -o gen-unicode
 *   ./gen-unicode | gzip -9n > ../../crates/okkama-runtime-core/data/unicode.dat
 * Программа отказывается работать, если ICU даёт не Unicode 17.0.
 */
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unicode/uchar.h>
#include <unicode/uversion.h>

#define LAST 0x10FFFF

static int is_mark(UChar32 c) { return (U_GET_GC_MASK(c) & U_GC_M_MASK) != 0; }
static int is_ignorable(UChar32 c) { return u_hasBinaryProperty(c, UCHAR_DEFAULT_IGNORABLE_CODE_POINT); }
static int is_regional(UChar32 c) { return u_hasBinaryProperty(c, UCHAR_REGIONAL_INDICATOR); }
static int is_emoji(UChar32 c) { return u_hasBinaryProperty(c, UCHAR_EMOJI); }
static int is_pictographic(UChar32 c) { return u_hasBinaryProperty(c, UCHAR_EXTENDED_PICTOGRAPHIC); }
static int is_presentation(UChar32 c) { return u_hasBinaryProperty(c, UCHAR_EMOJI_PRESENTATION); }
static int is_wide(UChar32 c) { return u_getIntPropertyValue(c, UCHAR_EAST_ASIAN_WIDTH) == U_EA_WIDE; }
static int is_full(UChar32 c) { return u_getIntPropertyValue(c, UCHAR_EAST_ASIAN_WIDTH) == U_EA_FULLWIDTH; }

static void ranges(const char *tag, int (*has)(UChar32)) {
    UChar32 from = -1;
    for (UChar32 c = 0; c <= LAST + 1; c++) {
        int in = c <= LAST && has(c);
        if (in && from < 0) from = c;
        if (!in && from >= 0) {
            printf("R %s %X %X\n", tag, (unsigned)from, (unsigned)(c - 1));
            from = -1;
        }
    }
}

/* Unicode::UCD отдаёт значение как в UnicodeData.txt: целое или n/d. */
static void numeric(char *out, size_t size, double value) {
    for (long denominator = 1; denominator <= 1000; denominator++) {
        double scaled = value * (double)denominator;
        if (fabs(scaled - nearbyint(scaled)) < 1e-6) {
            long long numerator = llround(scaled);
            if (denominator == 1)
                snprintf(out, size, "%lld", numerator);
            else
                snprintf(out, size, "%lld/%ld", numerator, denominator);
            return;
        }
    }
    fprintf(stderr, "no fraction for %.17g\n", value);
    exit(1);
}

struct run {
    char kind;
    char prefix[64];
    UChar32 from, to;
};
static struct run runs[256];
static int run_count;

static void run(char kind, const char *prefix, UChar32 c) {
    struct run *last = run_count ? &runs[run_count - 1] : NULL;
    if (last && last->kind == kind && strcmp(last->prefix, prefix) == 0 && last->to == c - 1) {
        last->to = c;
        return;
    }
    if (run_count == (int)(sizeof runs / sizeof runs[0])) {
        fprintf(stderr, "too many runs\n");
        exit(1);
    }
    struct run *next = &runs[run_count++];
    next->kind = kind;
    snprintf(next->prefix, sizeof next->prefix, "%s", prefix);
    next->from = next->to = c;
}

int main(void) {
    UVersionInfo version;
    u_getUnicodeVersion(version);
    if (version[0] != 17 || version[1] != 0) {
        fprintf(stderr, "ICU has Unicode %d.%d, need 17.0 (icu4c@78)\n", version[0], version[1]);
        return 1;
    }
    printf("V %d.%d.%d\n", version[0], version[1], version[2]);
    ranges("M", is_mark);
    ranges("DI", is_ignorable);
    ranges("RI", is_regional);
    ranges("E", is_emoji);
    ranges("EP", is_pictographic);
    ranges("EPres", is_presentation);
    ranges("W", is_wide);
    ranges("F", is_full);

    UChar32 from = 0;
    for (UChar32 c = 1; c <= LAST + 1; c++) {
        if (c <= LAST && u_charType(c) == u_charType(from)) continue;
        const char *name = u_getPropertyValueName(UCHAR_GENERAL_CATEGORY, u_charType(from), U_SHORT_PROPERTY_NAME);
        printf("G %s %X %X\n", name, (unsigned)from, (unsigned)(c - 1));
        from = c;
    }
    from = 0;
    for (UChar32 c = 1; c <= LAST + 1; c++) {
        if (c <= LAST && u_getCombiningClass(c) == u_getCombiningClass(from)) continue;
        if (u_getCombiningClass(from))
            printf("C %d %X %X\n", u_getCombiningClass(from), (unsigned)from, (unsigned)(c - 1));
        from = c;
    }

    for (UChar32 c = 0; c <= LAST; c++) {
        if (c >= 0xD800 && c <= 0xDFFF) continue;
        if (u_charType(c) == U_UNASSIGNED) continue;
        double value = u_getNumericValue(c);
        if (value != U_NO_NUMERIC_VALUE) {
            char text[64];
            numeric(text, sizeof text, value);
            printf("N %X %s\n", (unsigned)c, text);
        }
        /* Like Unicode::UCD, a correction alias (NameAliases.txt) replaces
         * the immutable Name: U+01A2 is LATIN CAPITAL LETTER GHA. */
        char name[256];
        UErrorCode status = U_ZERO_ERROR;
        int32_t length = u_charName(c, U_CHAR_NAME_ALIAS, name, sizeof name, &status);
        if (U_FAILURE(status) || length == 0) {
            status = U_ZERO_ERROR;
            length = u_charName(c, U_UNICODE_CHAR_NAME, name, sizeof name, &status);
        }
        if (U_FAILURE(status) || length == 0) continue;
        if (strncmp(name, "HANGUL SYLLABLE ", 16) == 0) {
            run('H', "", c);
            continue;
        }
        char *dash = strrchr(name, '-');
        if (dash && dash[1] && strtoul(dash + 1, NULL, 16) == (unsigned long)c &&
            strspn(dash + 1, "0123456789ABCDEF") == strlen(dash + 1)) {
            char prefix[64];
            snprintf(prefix, sizeof prefix, "%.*s", (int)(dash - name), name);
            for (char *p = prefix; *p; p++)
                if (*p == ' ') *p = '_';
            run('A', prefix, c);
            continue;
        }
        printf("M %X %s\n", (unsigned)c, name);
    }
    for (int i = 0; i < run_count; i++) {
        if (runs[i].kind == 'A')
            printf("A %s %X %X\n", runs[i].prefix, (unsigned)runs[i].from, (unsigned)runs[i].to);
        else
            printf("H %X %X\n", (unsigned)runs[i].from, (unsigned)runs[i].to);
    }
    return 0;
}
