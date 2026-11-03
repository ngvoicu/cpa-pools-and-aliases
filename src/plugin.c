/*
 * cpa-pools-and-aliases — CLIProxyAPI plugin
 *
 * Serves a single management UI page (menu resource) that edits CPA's *native*
 * model aliases across every credential in one place. The plugin itself does not
 * touch routing: the page talks to the regular /v8/management API with the
 * operator's management key, and CPA keeps doing what it already does with aliases.
 *
 * C ABI: see CLIProxyAPI examples/plugin (ABI version 1, JSON envelopes).
 */
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "page.h" /* generated: static const char PAGE_B64[] */

#if defined(_WIN32)
#define CLIPROXY_EXPORT __declspec(dllexport)
#else
#define CLIPROXY_EXPORT __attribute__((visibility("default")))
#endif

#ifndef PLUGIN_VERSION
#define PLUGIN_VERSION "0.0.0-dev"
#endif

#define ABI_VERSION 1
#define PLUGIN_NAME "Pools and Aliases"
#define PLUGIN_REPO "https://github.com/ngvoicu/cpa-pools-and-aliases"
#define PLUGIN_LOGO "https://raw.githubusercontent.com/ngvoicu/cpa-pools-and-aliases/main/assets/logo.svg"

typedef struct {
	void* ptr;
	size_t len;
} cliproxy_buffer;

typedef int (*cliproxy_host_call_fn)(void*, const char*, const uint8_t*, size_t, cliproxy_buffer*);
typedef void (*cliproxy_host_free_fn)(void*, size_t);

typedef struct {
	uint32_t abi_version;
	void* host_ctx;
	cliproxy_host_call_fn call;
	cliproxy_host_free_fn free_buffer;
} cliproxy_host_api;

typedef int (*cliproxy_plugin_call_fn)(const char*, const uint8_t*, size_t, cliproxy_buffer*);
typedef void (*cliproxy_plugin_free_fn)(void*, size_t);
typedef void (*cliproxy_plugin_shutdown_fn)(void);

typedef struct {
	uint32_t abi_version;
	cliproxy_plugin_call_fn call;
	cliproxy_plugin_free_fn free_buffer;
	cliproxy_plugin_shutdown_fn shutdown;
} cliproxy_plugin_api;

static const char REGISTER_RESULT[] =
	"{\"ok\":true,\"result\":{\"schema_version\":1,\"metadata\":{"
	"\"Name\":\"" PLUGIN_NAME "\","
	"\"Version\":\"" PLUGIN_VERSION "\","
	"\"Author\":\"ngvoicu\","
	"\"GitHubRepository\":\"" PLUGIN_REPO "\","
	"\"Logo\":\"" PLUGIN_LOGO "\","
	"\"ConfigFields\":[]},"
	"\"capabilities\":{\"management_api\":true}}}";

static const char MANAGEMENT_REGISTER_RESULT[] =
	"{\"ok\":true,\"result\":{\"resources\":[{"
	"\"Path\":\"/ui\","
	"\"Menu\":\"" PLUGIN_NAME "\","
	"\"Description\":\"Edit model aliases for every subscription and API key in one place, and build model pools from them.\""
	"}]}}";

static const char OK_EMPTY[] = "{\"ok\":true,\"result\":{}}";

static const char PAGE_PREFIX[] =
	"{\"ok\":true,\"result\":{\"StatusCode\":200,\"Headers\":{"
	"\"content-type\":[\"text/html; charset=utf-8\"],"
	"\"cache-control\":[\"no-store\"],"
	"\"x-content-type-options\":[\"nosniff\"]"
	"},\"Body\":\"";
static const char PAGE_SUFFIX[] = "\"}}";

static void write_bytes(cliproxy_buffer* response, const char* text, size_t len) {
	if (response == NULL || text == NULL) {
		return;
	}
	void* ptr = malloc(len);
	if (ptr == NULL) {
		response->ptr = NULL;
		response->len = 0;
		return;
	}
	memcpy(ptr, text, len);
	response->ptr = ptr;
	response->len = len;
}

static void write_text(cliproxy_buffer* response, const char* text) {
	write_bytes(response, text, strlen(text));
}

static void write_page(cliproxy_buffer* response) {
	size_t prefix_len = sizeof(PAGE_PREFIX) - 1;
	size_t body_len = sizeof(PAGE_B64) - 1;
	size_t suffix_len = sizeof(PAGE_SUFFIX) - 1;
	size_t total = prefix_len + body_len + suffix_len;
	char* out = (char*)malloc(total);
	if (out == NULL) {
		write_text(response, "{\"ok\":false,\"error\":{\"code\":\"oom\",\"message\":\"out of memory\"}}");
		return;
	}
	memcpy(out, PAGE_PREFIX, prefix_len);
	memcpy(out + prefix_len, PAGE_B64, body_len);
	memcpy(out + prefix_len + body_len, PAGE_SUFFIX, suffix_len);
	response->ptr = out;
	response->len = total;
}

static int plugin_call(const char* method, const uint8_t* request, size_t request_len, cliproxy_buffer* response) {
	(void)request;
	(void)request_len;
	if (response != NULL) {
		response->ptr = NULL;
		response->len = 0;
	}
	if (method == NULL) {
		write_text(response, "{\"ok\":false,\"error\":{\"code\":\"invalid_method\",\"message\":\"method is required\"}}");
		return 1;
	}
	if (strcmp(method, "plugin.register") == 0 || strcmp(method, "plugin.reconfigure") == 0) {
		write_text(response, REGISTER_RESULT);
		return 0;
	}
	if (strcmp(method, "management.register") == 0) {
		write_text(response, MANAGEMENT_REGISTER_RESULT);
		return 0;
	}
	if (strcmp(method, "management.handle") == 0) {
		/* Only one resource is registered (/ui), so every call serves the page. */
		write_page(response);
		return 0;
	}
	if (strcmp(method, "plugin.quiesce") == 0 || strcmp(method, "plugin.shutdown") == 0) {
		write_text(response, OK_EMPTY);
		return 0;
	}
	write_text(response, "{\"ok\":false,\"error\":{\"code\":\"unknown_method\",\"message\":\"unknown method\"}}");
	return 0;
}

static void plugin_free(void* ptr, size_t len) {
	(void)len;
	free(ptr);
}

static void plugin_shutdown(void) {}

CLIPROXY_EXPORT int cliproxy_plugin_init(const cliproxy_host_api* host, cliproxy_plugin_api* plugin) {
	(void)host;
	if (plugin == NULL) {
		return 1;
	}
	plugin->abi_version = ABI_VERSION;
	plugin->call = plugin_call;
	plugin->free_buffer = plugin_free;
	plugin->shutdown = plugin_shutdown;
	return 0;
}
