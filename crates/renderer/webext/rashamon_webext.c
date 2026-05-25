#include <glib.h>
#include <webkit2/webkit-web-extension.h>

static gchar *g_rules_block_csv = NULL;
static gchar *g_rules_allow_csv = NULL;

static gboolean csv_contains_host(const gchar *csv, const gchar *uri) {
    if (!csv || !*csv || !uri || !*uri) {
        return FALSE;
    }
    g_auto(GStrv) tokens = g_strsplit(csv, ",", -1);
    for (gint i = 0; tokens && tokens[i]; i++) {
        const gchar *token = tokens[i];
        if (!token || !*token) {
            continue;
        }
        if (g_strstr_len(uri, -1, token) != NULL) {
            return TRUE;
        }
    }
    return FALSE;
}

static gboolean should_block_uri(const gchar *uri) {
    if (!uri || *uri == '\0') {
        return FALSE;
    }
    if (csv_contains_host(g_rules_allow_csv, uri)) {
        return FALSE;
    }
    if (csv_contains_host(g_rules_block_csv, uri)) {
        return TRUE;
    }
    return g_strstr_len(uri, -1, "doubleclick.net") != NULL
        || g_strstr_len(uri, -1, "googlesyndication.com") != NULL;
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
    if (!should_block_uri(uri)) {
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
            g_printerr("[adblock-webext] ping -> pong page=%u\n", webkit_web_page_get_id(page));
        }
        return TRUE;
    }

    if (g_strcmp0(name, "rashamon-webext-set-rules") == 0) {
        GVariant *params = webkit_user_message_get_parameters(message);
        if (params) {
            const gchar *block_csv = NULL;
            const gchar *allow_csv = NULL;
            if (g_variant_is_of_type(params, G_VARIANT_TYPE("(ss)"))) {
                g_variant_get(params, "(&s&s)", &block_csv, &allow_csv);
                g_free(g_rules_block_csv);
                g_free(g_rules_allow_csv);
                g_rules_block_csv = g_strdup(block_csv ? block_csv : "");
                g_rules_allow_csv = g_strdup(allow_csv ? allow_csv : "");
                if (g_getenv("RASHAMON_DEBUG")) {
                    g_printerr("[adblock-webext] rules updated block=%s allow=%s\n",
                               g_rules_block_csv, g_rules_allow_csv);
                }
            }
        }
        WebKitUserMessage *reply = webkit_user_message_new("rashamon-webext-rules-ok", NULL);
        webkit_user_message_send_reply(message, reply);
        return TRUE;
    }

    if (g_strcmp0(name, "rashamon-webext-probe-url") == 0) {
        GVariant *params = webkit_user_message_get_parameters(message);
        const gchar *uri = NULL;
        if (params && g_variant_is_of_type(params, G_VARIANT_TYPE("(s)"))) {
            g_variant_get(params, "(&s)", &uri);
        }
        gboolean blocked = should_block_uri(uri);
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
        g_printerr("[adblock-webext] page-created id=%u\n", webkit_web_page_get_id(page));
    }
}

G_MODULE_EXPORT void webkit_web_extension_initialize(WebKitWebExtension *extension) {
    if (g_getenv("RASHAMON_DEBUG")) {
        g_printerr("[adblock-webext] initialize\n");
    }
    g_signal_connect(extension, "page-created", G_CALLBACK(on_page_created), NULL);
}
