/* Smoke-test for the C ABI surface. Compile against libcallbook.so/dylib/dll.
 *
 * Build (after `cargo build --release`):
 *   cc -o c_smoke crates/callbook/tests/c_abi.c \
 *      -I crates/callbook/include \
 *      -L target/release -l callbook
 *
 * Run with CALLBOOK_DB pointing at a real install:
 *   CALLBOOK_DB=~/HAMCALL ./c_smoke W1AW
 */

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "callbook.h"

int main(int argc, char **argv) {
    if (argc < 2) {
        fprintf(stderr, "usage: %s <CALLSIGN>\n", argv[0]);
        return 2;
    }
    const char *db_path = getenv("CALLBOOK_DB");
    if (!db_path) {
        fprintf(stderr, "set CALLBOOK_DB to the install root\n");
        return 2;
    }

    callbook_db *db = NULL;
    int rc = callbook_open(db_path, &db);
    if (rc != 0 || !db) {
        fprintf(stderr, "open failed: %d\n", rc);
        return 1;
    }

    callbook_lookup_result *result = NULL;
    rc = callbook_lookup_modern(db, argv[1], &result);
    if (rc != 0 || !result) {
        fprintf(stderr, "modern lookup failed: %d\n", rc);
        callbook_close(db);
        return 1;
    }

    printf("query: %s\n", callbook_result_query(result));
    printf("status: %d\n", callbook_result_status(result));
    const callbook_snapshot *current = callbook_result_current(result);
    if (current) {
        printf("name: %s\n", callbook_snapshot_field(current, callbook_modern_field_Name));
        printf("country: %s\n", callbook_snapshot_field(current, callbook_modern_field_Country));
        printf("grid: %s\n", callbook_snapshot_field(current, callbook_modern_field_Grid));
        printf("history: %d\n", callbook_result_history_len(result));
        printf("interest_codes_raw: %s\n", callbook_snapshot_interest_codes_raw(current));
        int interest_len = callbook_snapshot_interest_len(current);
        printf("interests: %d\n", interest_len);
        for (int i = 0; i < interest_len; i++) {
            const callbook_interest *interest = callbook_snapshot_interest_get(current, i);
            printf("  %s %s: %s\n",
                   callbook_interest_code(interest),
                   callbook_interest_category(interest),
                   callbook_interest_label(interest));
        }
    }

    int need = callbook_lookup_json_required_len(db, argv[1]);
    if (need > 0) {
        char *json = (char *)malloc((size_t)need);
        if (!json) {
            fprintf(stderr, "malloc failed\n");
            callbook_result_free(result);
            callbook_close(db);
            return 1;
        }
        int n = callbook_lookup_json(db, argv[1], json, need);
        if (n >= 0) {
            printf("json_bytes: %d\n", n);
        }
        free(json);
    }

    callbook_result_free(result);
    callbook_close(db);
    return 0;
}
