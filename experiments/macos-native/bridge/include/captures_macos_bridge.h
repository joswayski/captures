#ifndef CAPTURES_MACOS_BRIDGE_H
#define CAPTURES_MACOS_BRIDGE_H

#ifdef __cplusplus
extern "C" {
#endif

char *captures_native_request(const char *json);
void captures_native_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
