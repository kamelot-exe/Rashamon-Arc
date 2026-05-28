#include <glib.h>
#include <string.h>
#include <webkit2/webkit-web-extension.h>

typedef struct {
    gboolean enabled;
    GPtrArray *block_domains;
    GPtrArray *block_substrings;
    GPtrArray *allow_domains;
} Ruleset;

static Ruleset *g_rules = NULL;

static Ruleset *ruleset_new(void) {
    Ruleset *rules = g_new0(Ruleset, 1);
    rules->enabled = TRUE;
    rules->block_domains = g_ptr_array_new_with_free_func(g_free);
    rules->block_substrings = g_ptr_array_new_with_free_func(g_free);
    rules->allow_domains = g_ptr_array_new_with_free_func(g_free);
    return rules;
}

static void ruleset_free(Ruleset *rules) {
    if (!rules) {
        return;
    }
    g_ptr_array_unref(rules->block_domains);
    g_ptr_array_unref(rules->block_substrings);
    g_ptr_array_unref(rules->allow_domains);
    g_free(rules);
}
G_DEFINE_AUTOPTR_CLEANUP_FUNC(Ruleset, ruleset_free)

static gchar *lower_trim_dup(const gchar *value) {
    if (!value) {
        return NULL;
    }
    g_autofree gchar *trimmed = g_strdup(value);
    g_strstrip(trimmed);
    if (*trimmed == '\0' || strchr(trimmed, '\n') || strchr(trimmed, '\r')) {
        return NULL;
    }
    return g_ascii_strdown(trimmed, -1);
}

static gchar *host_from_uri(const gchar *uri) {
    if (!uri || *uri == '\0') {
        return NULL;
    }
    g_autofree gchar *lower = g_ascii_strdown(uri, -1);
    const gchar *start = strstr(lower, "://");
    start = start ? start + 3 : lower;
    const gchar *after_user = strrchr(start, '@');
    if (after_user) {
        start = after_user + 1;
    }
    gsize len = strcspn(start, "/?#");
    if (len == 0) {
        return NULL;
    }
    g_autofree gchar *host_port = g_strndup(start, len);
    gchar *colon = strrchr(host_port, ':');
    if (colon && colon[1] != '\0') {
        gboolean numeric_port = TRUE;
        for (gchar *p = colon + 1; *p; p++) {
            if (!g_ascii_isdigit(*p)) {
                numeric_port = FALSE;
                break;
            }
        }
        if (numeric_port) {
            *colon = '\0';
        }
    }
    g_strstrip(host_port);
    while (*host_port == '.') {
        memmove(host_port, host_port + 1, strlen(host_port));
    }
    if (*host_port == '\0') {
        return NULL;
    }
    return g_strdup(host_port);
}

static gboolean host_matches_domain(const gchar *host, const gchar *domain) {
    if (!host || !domain || !*host || !*domain) {
        return FALSE;
    }
    if (g_strcmp0(host, domain) == 0) {
        return TRUE;
    }
    gsize host_len = strlen(host);
    gsize domain_len = strlen(domain);
    return host_len > domain_len
        && host[host_len - domain_len - 1] == '.'
        && g_strcmp0(host + host_len - domain_len, domain) == 0;
}

static gboolean domain_array_matches(GPtrArray *domains, const gchar *host) {
    if (!domains || !host) {
        return FALSE;
    }
    for (guint i = 0; i < domains->len; i++) {
        const gchar *domain = g_ptr_array_index(domains, i);
        if (host_matches_domain(host, domain)) {
            return TRUE;
        }
    }
    return FALSE;
}

static gboolean substring_array_matches(GPtrArray *patterns, const gchar *uri_lc) {
    if (!patterns || !uri_lc) {
        return FALSE;
    }
    for (guint i = 0; i < patterns->len; i++) {
        const gchar *pattern = g_ptr_array_index(patterns, i);
        if (g_strstr_len(uri_lc, -1, pattern) != NULL) {
            return TRUE;
        }
    }
    return FALSE;
}

static gboolean parse_rules_payload(const gchar *payload, Ruleset **out_rules) {
    if (!payload || !out_rules) {
        return FALSE;
    }
    g_autoptr(Ruleset) rules = ruleset_new();
    gboolean saw_version = FALSE;
    gboolean saw_enabled = FALSE;
    g_auto(GStrv) lines = g_strsplit(payload, "\n", -1);
    for (gint i = 0; lines && lines[i]; i++) {
        gchar *line = lines[i];
        g_strstrip(line);
        if (*line == '\0') {
            continue;
        }
        gchar *eq = strchr(line, '=');
        if (!eq) {
            return FALSE;
        }
        *eq = '\0';
        const gchar *key = line;
        const gchar *value = eq + 1;
        if (g_strcmp0(key, "version") == 0) {
            if (g_strcmp0(value, "1") != 0) {
                return FALSE;
            }
            saw_version = TRUE;
        } else if (g_strcmp0(key, "enabled") == 0) {
            if (g_strcmp0(value, "1") == 0) {
                rules->enabled = TRUE;
            } else if (g_strcmp0(value, "0") == 0) {
                rules->enabled = FALSE;
            } else {
                return FALSE;
            }
            saw_enabled = TRUE;
        } else if (g_strcmp0(key, "block-domain") == 0) {
            gchar *domain = lower_trim_dup(value);
            if (!domain) {
                return FALSE;
            }
            g_ptr_array_add(rules->block_domains, domain);
        } else if (g_strcmp0(key, "block-substring") == 0) {
            gchar *pattern = lower_trim_dup(value);
            if (!pattern) {
                return FALSE;
            }
            g_ptr_array_add(rules->block_substrings, pattern);
        } else if (g_strcmp0(key, "allow-domain") == 0) {
            gchar *domain = lower_trim_dup(value);
            if (!domain) {
                return FALSE;
            }
            g_ptr_array_add(rules->allow_domains, domain);
        } else {
            return FALSE;
        }
    }
    if (!saw_version || !saw_enabled) {
        return FALSE;
    }
    *out_rules = g_steal_pointer(&rules);
    return TRUE;
}

static gboolean should_block_uri(const gchar *uri, const gchar *page_uri) {
    if (!uri || *uri == '\0') {
        return FALSE;
    }
    Ruleset *rules = g_rules;
    if (!rules || !rules->enabled) {
        return FALSE;
    }
    g_autofree gchar *uri_lc = g_ascii_strdown(uri, -1);
    g_autofree gchar *host = host_from_uri(uri);
    g_autofree gchar *page_host = host_from_uri(page_uri);
    if (domain_array_matches(rules->allow_domains, host)
        || domain_array_matches(rules->allow_domains, page_host)) {
        return FALSE;
    }
    return domain_array_matches(rules->block_domains, host)
        || substring_array_matches(rules->block_substrings, uri_lc);
}

static void report_blocked_to_view(WebKitWebPage *page, const gchar *uri) {
    if (!page || !uri) {
        return;
    }
    const gchar *page_uri = webkit_web_page_get_uri(page);
    GVariant *params = g_variant_new("(sss)", uri, page_uri ? page_uri : "", "send-request");
    WebKitUserMessage *msg = webkit_user_message_new("rashamon-webext-blocked", params);
    webkit_web_page_send_message_to_view(page, msg, NULL, NULL, NULL);
}

static gboolean on_page_send_request(
    WebKitWebPage *page,
    WebKitURIRequest *request,
    WebKitURIResponse *redirected_response,
    gpointer user_data
) {
    (void)page;
    (void)redirected_response;
    (void)user_data;
    const gchar *uri = webkit_uri_request_get_uri(request);
    if (!should_block_uri(uri, webkit_web_page_get_uri(page))) {
        return FALSE;
    }
    report_blocked_to_view(page, uri);
    if (g_getenv("RASHAMON_DEBUG")) {
        g_printerr("[adblock-webext] blocked request uri=%s\n", uri ? uri : "(null)");
    }
    return TRUE;
}

static gboolean on_page_user_message_received(
    WebKitWebPage *page,
    WebKitUserMessage *message,
    gpointer user_data
) {
    (void)user_data;
    if (!message) {
        return FALSE;
    }
    const gchar *name = webkit_user_message_get_name(message);
    if (!name) {
        return FALSE;
    }

    if (g_strcmp0(name, "rashamon-webext-ping") == 0) {
        WebKitUserMessage *reply = webkit_user_message_new("rashamon-webext-pong", NULL);
        webkit_user_message_send_reply(message, reply);
        if (g_getenv("RASHAMON_DEBUG")) {
            g_printerr(
                "[adblock-webext] ping -> pong page=%" G_GUINT64_FORMAT "\n",
                webkit_web_page_get_id(page)
            );
        }
        return TRUE;
    }

    if (g_strcmp0(name, "rashamon-webext-set-rules") == 0) {
        GVariant *params = webkit_user_message_get_parameters(message);
        const gchar *payload = NULL;
        if (params && g_variant_is_of_type(params, G_VARIANT_TYPE("(s)"))) {
            g_variant_get(params, "(&s)", &payload);
        }
        Ruleset *new_rules = NULL;
        if (parse_rules_payload(payload, &new_rules)) {
            ruleset_free(g_rules);
            g_rules = new_rules;
            if (g_getenv("RASHAMON_DEBUG")) {
                g_printerr(
                    "[adblock-webext] rules updated domains=%u substrings=%u allow=%u enabled=%d\n",
                    g_rules->block_domains->len,
                    g_rules->block_substrings->len,
                    g_rules->allow_domains->len,
                    g_rules->enabled
                );
            }
            WebKitUserMessage *reply = webkit_user_message_new("rashamon-webext-rules-ok", NULL);
            webkit_user_message_send_reply(message, reply);
        } else {
            if (g_getenv("RASHAMON_DEBUG")) {
                g_printerr("[adblock-webext] invalid rules payload; keeping previous rules\n");
            }
            WebKitUserMessage *reply = webkit_user_message_new("rashamon-webext-rules-error", NULL);
            webkit_user_message_send_reply(message, reply);
        }
        return TRUE;
    }

    if (g_strcmp0(name, "rashamon-webext-probe-url") == 0) {
        GVariant *params = webkit_user_message_get_parameters(message);
        const gchar *uri = NULL;
        if (params && g_variant_is_of_type(params, G_VARIANT_TYPE("(s)"))) {
            g_variant_get(params, "(&s)", &uri);
        }
        gboolean blocked = should_block_uri(uri, webkit_web_page_get_uri(page));
        if (blocked) {
            report_blocked_to_view(page, uri ? uri : "about:blank");
        }
        WebKitUserMessage *reply = webkit_user_message_new(
            blocked ? "rashamon-webext-probe-blocked" : "rashamon-webext-probe-clear",
            NULL
        );
        webkit_user_message_send_reply(message, reply);
        return TRUE;
    }

    return FALSE;
}

static void on_page_created(
    WebKitWebExtension *extension,
    WebKitWebPage *page,
    gpointer user_data
) {
    (void)extension;
    (void)user_data;
    g_signal_connect(page, "send-request", G_CALLBACK(on_page_send_request), NULL);
    g_signal_connect(page, "user-message-received", G_CALLBACK(on_page_user_message_received), NULL);
    if (g_getenv("RASHAMON_DEBUG")) {
        g_printerr(
            "[adblock-webext] page-created id=%" G_GUINT64_FORMAT "\n",
            webkit_web_page_get_id(page)
        );
    }
}

G_MODULE_EXPORT void webkit_web_extension_initialize(WebKitWebExtension *extension) {
    if (g_getenv("RASHAMON_DEBUG")) {
        g_printerr("[adblock-webext] initialize\n");
    }
    g_signal_connect(extension, "page-created", G_CALLBACK(on_page_created), NULL);
}
