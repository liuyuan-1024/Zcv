#include <stdio.h>

#define MAX_RETRIES 3

typedef struct {
    const char *name;
    int enabled;
} Config;

static int run(Config config) {
    for (int attempt = 0; attempt < MAX_RETRIES; attempt++) {
        if (config.enabled) printf("%s: %d\n", config.name, attempt);
    }
    return config.enabled;
}

int main(void) {
    Config config = {"zcv", 1};
    return run(config) ? 0 : 1;
}
