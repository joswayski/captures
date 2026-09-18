#include <gtk/gtk.h>
#include <json-glib/json-glib.h>
#include <math.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>

typedef enum { SCENE_PREFERENCES, SCENE_HISTORY, SCENE_HUD, SCENE_PREVIEW, SCENE_EDITOR, SCENE_IDLE } Scene;

typedef struct {
  Scene scene;
  const char *appearance;
  const char *theme;
  guint history_count;
  gboolean exercise;
  gboolean floating;
  gboolean reduced_motion;
  double quit_after;
  double screenshot_after;
  const char *screenshot;
} Options;

typedef struct {
  GHashTable *colors;
  GHashTable *numbers;
} Tokens;

typedef struct _CapturesWorkbench {
  GtkWidget parent;
  Options options;
  Tokens tokens;
  GtkWidget *search;
  GtkWidget *annotation;
  GdkTexture *texture;
  gint64 started;
  gint64 animation_started;
  guint snapshots;
  guint actions;
  guint animation_tick;
  guint animation_timeout;
  guint screenshot_source;
  double scroll;
  double zoom;
  double rotation;
  double pan_x;
  double pan_y;
  gboolean paused;
  gboolean muted;
  gboolean preview_hidden;
  guint selected_row;
} CapturesWorkbench;

typedef GtkWidgetClass CapturesWorkbenchClass;
G_DEFINE_TYPE(CapturesWorkbench, captures_workbench, GTK_TYPE_WIDGET)

static const char *scene_name(Scene scene) {
  switch (scene) {
    case SCENE_PREFERENCES: return "preferences";
    case SCENE_HISTORY: return "history";
    case SCENE_HUD: return "hud";
    case SCENE_PREVIEW: return "preview";
    case SCENE_EDITOR: return "editor";
    case SCENE_IDLE: return "idle";
  }
  return "unknown";
}

static void emit(const char *event, const char *format, ...) {
  g_autoptr(JsonBuilder) builder = json_builder_new();
  json_builder_begin_object(builder);
  json_builder_set_member_name(builder, "schema"); json_builder_add_int_value(builder, 1);
  json_builder_set_member_name(builder, "pid"); json_builder_add_int_value(builder, getpid());
  json_builder_set_member_name(builder, "event"); json_builder_add_string_value(builder, event);
  json_builder_set_member_name(builder, "detail");
  json_builder_begin_object(builder);
  va_list args;
  va_start(args, format);
  const char *key = format;
  while (key) {
    char kind = key[0];
    const char *name = key + 2;
    json_builder_set_member_name(builder, name);
    if (kind == 's') json_builder_add_string_value(builder, va_arg(args, const char *));
    else if (kind == 'i') json_builder_add_int_value(builder, va_arg(args, int));
    else if (kind == 'd') json_builder_add_double_value(builder, va_arg(args, double));
    else if (kind == 'b') json_builder_add_boolean_value(builder, va_arg(args, int));
    key = va_arg(args, const char *);
  }
  va_end(args);
  json_builder_end_object(builder);
  json_builder_end_object(builder);
  g_autoptr(JsonGenerator) generator = json_generator_new();
  g_autoptr(JsonNode) root = json_builder_get_root(builder);
  json_generator_set_root(generator, root);
  g_autofree char *json = json_generator_to_data(generator, NULL);
  g_print("%s\n", json);
  fflush(stdout);
}

static GdkRGBA rgba(CapturesWorkbench *self, const char *name) {
  GdkRGBA *value = g_hash_table_lookup(self->tokens.colors, name);
  if (!value) g_error("Missing color token %s", name);
  return *value;
}

static double number(CapturesWorkbench *self, const char *name) {
  double *value = g_hash_table_lookup(self->tokens.numbers, name);
  if (!value) g_error("Missing number token %s", name);
  return *value;
}

static void rounded(GtkSnapshot *snapshot, graphene_rect_t rect, float radius, GdkRGBA color) {
  GskRoundedRect rounded_rect;
  gsk_rounded_rect_init_from_rect(&rounded_rect, &rect, radius);
  gtk_snapshot_push_rounded_clip(snapshot, &rounded_rect);
  gtk_snapshot_append_color(snapshot, &color, &rect);
  gtk_snapshot_pop(snapshot);
}

static void border(GtkSnapshot *snapshot, graphene_rect_t rect, float radius, GdkRGBA fill, GdkRGBA line) {
  rounded(snapshot, rect, radius, fill);
  GskRoundedRect outline;
  gsk_rounded_rect_init_from_rect(&outline, &rect, radius);
  float widths[4] = { 1, 1, 1, 1 };
  GdkRGBA colors[4] = { line, line, line, line };
  gtk_snapshot_append_border(snapshot, &outline, widths, colors);
}

static void text(CapturesWorkbench *self, GtkSnapshot *snapshot, const char *value,
                 float x, float y, float size, const char *color, PangoWeight weight) {
  PangoLayout *layout = gtk_widget_create_pango_layout(GTK_WIDGET(self), value);
  PangoFontDescription *font = pango_font_description_new();
  pango_font_description_set_family(font, "Cantarell, Segoe UI, sans-serif");
  pango_font_description_set_absolute_size(font, size * PANGO_SCALE);
  pango_font_description_set_weight(font, weight);
  pango_layout_set_font_description(layout, font);
  GdkRGBA ink = rgba(self, color);
  gtk_snapshot_save(snapshot);
  gtk_snapshot_translate(snapshot, &GRAPHENE_POINT_INIT(x, y));
  gtk_snapshot_append_layout(snapshot, layout, &ink);
  gtk_snapshot_restore(snapshot);
  pango_font_description_free(font);
  g_object_unref(layout);
}

static GdkTexture *fixture_texture(void) {
  const int width = 2048, height = 1152, stride = width * 4;
  guchar *pixels = g_malloc(stride * height);
  for (int y = 0; y < height; y++) for (int x = 0; x < width; x++) {
    double sx = x / (double)width * 284, sy = (1 - y / (double)height) * 160;
    guchar rgb[3];
    if (pow(sx - 233, 2) + pow(sy - 123, 2) < 225) memcpy(rgb, (guchar[]){245,189,74}, 3);
    else if (sx >= 38 && sx < 113 && sy >= 50 && sy < 130) memcpy(rgb, (guchar[]){217,84,105}, 3);
    else if (sy < 45) memcpy(rgb, (guchar[]){51,122,102}, 3);
    else memcpy(rgb, (guchar[]){31,69,107}, 3);
    guchar *p = pixels + y * stride + x * 4;
    p[0] = rgb[0]; p[1] = rgb[1]; p[2] = rgb[2]; p[3] = 255;
  }
  GBytes *bytes = g_bytes_new_take(pixels, stride * height);
  GdkTexture *texture = gdk_memory_texture_new(width, height, GDK_MEMORY_R8G8B8A8, bytes, stride);
  g_bytes_unref(bytes);
  return texture;
}

static void draw_chrome(CapturesWorkbench *self, GtkSnapshot *snapshot, int width, int height) {
  if (self->options.floating) return;
  GdkRGBA canvas = rgba(self, "surface-canvas");
  GdkRGBA sunken = rgba(self, "surface-sunken");
  gtk_snapshot_append_color(snapshot, &canvas, &GRAPHENE_RECT_INIT(0, 0, width, height));
  gtk_snapshot_append_color(snapshot, &sunken, &GRAPHENE_RECT_INIT(0, 0, 196, height));
  text(self, snapshot, "Captures", 20, 22, number(self, "text-xl"), "text", PANGO_WEIGHT_SEMIBOLD);
  const char *items[] = { "Preferences", "Capture history", "Recording controls", "Mini previews", "Editor probe" };
  Scene scenes[] = { SCENE_PREFERENCES, SCENE_HISTORY, SCENE_HUD, SCENE_PREVIEW, SCENE_EDITOR };
  for (int i = 0; i < 5; i++) {
    graphene_rect_t rect = GRAPHENE_RECT_INIT(12, 70 + i * 44, 172, 34);
    if (self->options.scene == scenes[i]) border(snapshot, rect, number(self, "r-md"), rgba(self, "surface-selected"), rgba(self, "theme-accent"));
    text(self, snapshot, items[i], 24, 79 + i * 44, number(self, "text-md"), "text", PANGO_WEIGHT_MEDIUM);
  }
  text(self, snapshot, "GTK4 snapshot fixture", 20, height - 62, number(self, "text-sm"), "text-muted", PANGO_WEIGHT_NORMAL);
  text(self, snapshot, "No capture access", 20, height - 38, number(self, "text-sm"), "text-muted", PANGO_WEIGHT_NORMAL);
  const char *title = items[self->options.scene > SCENE_EDITOR ? 0 : self->options.scene];
  text(self, snapshot, title, 220, 20, number(self, "text-xl"), "text", PANGO_WEIGHT_SEMIBOLD);
  if (self->options.scene != SCENE_EDITOR)
    text(self, snapshot, "Synthetic renderer probe, not functional parity", 220, 47, number(self, "text-sm"), "text-muted", PANGO_WEIGHT_NORMAL);
}

static void draw_preferences(CapturesWorkbench *self, GtkSnapshot *snapshot) {
  graphene_rect_t panel = GRAPHENE_RECT_INIT(220, 142, 752, 240);
  border(snapshot, panel, number(self, "r-xl"), rgba(self, "surface-raised"), rgba(self, "border"));
  text(self, snapshot, "Appearance", 238, 162, number(self, "text-lg"), "text", PANGO_WEIGHT_SEMIBOLD);
  text(self, snapshot, "Choose the interface appearance and accent color.", 238, 190, number(self, "text-md"), "text-muted", PANGO_WEIGHT_NORMAL);
  text(self, snapshot, "Interface theme", 238, 236, number(self, "text-md"), "text", PANGO_WEIGHT_NORMAL);
  const char *modes[] = { "Light", "Dark" };
  const char *mode_values[] = { "light", "dark" };
  for (int i = 0; i < 2; i++) {
    gboolean selected = !strcmp(self->options.appearance, mode_values[i]);
    graphene_rect_t r = GRAPHENE_RECT_INIT(746 + i * 96, 224, 86, 34);
    border(snapshot, r, number(self, "r-md"), rgba(self, selected ? "surface-selected" : "control"), rgba(self, selected ? "theme-accent" : "control-border"));
    text(self, snapshot, modes[i], 764 + i * 96, 233, number(self, "text-md"), "text", PANGO_WEIGHT_MEDIUM);
  }
  text(self, snapshot, "Accent color", 238, 286, number(self, "text-md"), "text", PANGO_WEIGHT_NORMAL);
  const char *themes[] = { "Mustard", "Ember", "Rose", "Violet", "Cobalt" };
  const char *theme_values[] = { "mustard", "ember", "rose", "violet", "cobalt" };
  for (int i = 0; i < 5; i++) {
    gboolean selected = !strcmp(self->options.theme, theme_values[i]);
    graphene_rect_t r = GRAPHENE_RECT_INIT(238 + i * 142, 318, 132, 38);
    border(snapshot, r, number(self, "r-md"), rgba(self, selected ? "surface-selected" : "control"), rgba(self, selected ? "theme-accent" : "control-border"));
    text(self, snapshot, themes[i], 256 + i * 142, 328, number(self, "text-md"), "text", PANGO_WEIGHT_MEDIUM);
  }
  panel = GRAPHENE_RECT_INIT(220, 400, 752, 180);
  border(snapshot, panel, number(self, "r-xl"), rgba(self, "surface-raised"), rgba(self, "border"));
  text(self, snapshot, "Capture", 238, 420, number(self, "text-lg"), "text", PANGO_WEIGHT_SEMIBOLD);
  text(self, snapshot, "Fixture state only. Installed Captures settings are untouched.", 238, 448, number(self, "text-md"), "text-muted", PANGO_WEIGHT_NORMAL);
  text(self, snapshot, "Automatically copy captures", 238, 492, number(self, "text-md"), "text", PANGO_WEIGHT_NORMAL);
  text(self, snapshot, "Show mini previews", 238, 536, number(self, "text-md"), "text", PANGO_WEIGHT_NORMAL);
  for (int i = 0; i < 2; i++) {
    graphene_rect_t toggle = GRAPHENE_RECT_INIT(876, 484 + i * 44, 72, 30);
    rounded(snapshot, toggle, 15, rgba(self, "theme-accent"));
    rounded(snapshot, GRAPHENE_RECT_INIT(922, 488 + i * 44, 22, 22), 11, rgba(self, "theme-accent-ink"));
  }
}

static void draw_history(CapturesWorkbench *self, GtkSnapshot *snapshot, int height) {
  char summary[96];
  g_snprintf(summary, sizeof(summary), "%u synthetic captures · image-backed virtual rows", self->options.history_count);
  text(self, snapshot, summary, 220, 86, number(self, "text-md"), "text-muted", PANGO_WEIGHT_NORMAL);
  if (!self->options.history_count) {
    text(self, snapshot, "No captures yet", 240, 180, number(self, "text-xl"), "text", PANGO_WEIGHT_SEMIBOLD);
    text(self, snapshot, "This fixture never reads capture history.", 240, 215, number(self, "text-md"), "text-muted", PANGO_WEIGHT_NORMAL);
    return;
  }
  const double row_h = 74;
  guint first = (guint)(self->scroll / row_h);
  guint visible = (height - 142) / row_h + 1;
  guint last = MIN(self->options.history_count, first + visible);
  gtk_snapshot_push_clip(snapshot, &GRAPHENE_RECT_INIT(220, 122, 752, height - 142));
  for (guint row = first; row < last; row++) {
    float y = 126 + row * row_h - self->scroll;
    graphene_rect_t card = GRAPHENE_RECT_INIT(220, y, 752, 66);
    border(snapshot, card, number(self, "r-lg"), rgba(self, row == self->selected_row ? "surface-selected" : "surface-raised"), rgba(self, row == self->selected_row ? "theme-accent" : "border"));
    if (row == self->selected_row)
      rounded(snapshot, GRAPHENE_RECT_INIT(224, y + 8, 4, 50), 2, rgba(self, "theme-accent"));
    gtk_snapshot_append_texture(snapshot, self->texture, &GRAPHENE_RECT_INIT(228, y + 8, 88, 50));
    char title[48]; g_snprintf(title, sizeof(title), "%s %u", row % 3 == 0 ? "Video" : row % 3 == 1 ? "Screenshot" : "GIF", row + 1);
    text(self, snapshot, title, 332, y + 13, number(self, "text-md"), "text", PANGO_WEIGHT_MEDIUM);
    text(self, snapshot, "Synthetic image · no file on disk", 332, y + 37, number(self, "text-sm"), "text-muted", PANGO_WEIGHT_NORMAL);
  }
  gtk_snapshot_pop(snapshot);
}

static void draw_glass(CapturesWorkbench *self, GtkSnapshot *snapshot, graphene_rect_t rect) {
  border(snapshot, rect, number(self, "r-xl"), rgba(self, "glass-strong"), rgba(self, "glass-border"));
}

static void draw_hud(CapturesWorkbench *self, GtkSnapshot *snapshot, int width, int height) {
  float x = self->options.floating ? 30 : 320, y = self->options.floating ? height / 2 - 60 : 270;
  graphene_rect_t hud = GRAPHENE_RECT_INIT(x, y, self->options.floating ? width - 60 : 550, 112);
  draw_glass(self, snapshot, hud);
  text(self, snapshot, self->paused ? "Ⅱ  Paused · 00:24" : "●  Recording · 00:24", x + 24, y + 20, number(self, "text-lg"), self->paused ? "glass-text" : "theme-signal", PANGO_WEIGHT_SEMIBOLD);
  text(self, snapshot, self->muted ? "Microphone muted · synthetic state" : "Microphone on · synthetic state", x + 24, y + 55, number(self, "text-sm"), "glass-text-muted", PANGO_WEIGHT_NORMAL);
  graphene_rect_t button = GRAPHENE_RECT_INIT(x + hud.size.width - 142, y + 32, 116, 38);
  border(snapshot, button, number(self, "r-md"), rgba(self, "glass-raised"), rgba(self, "glass-border-strong"));
  text(self, snapshot, self->paused ? "Resume" : "Pause", button.origin.x + 28, button.origin.y + 10, number(self, "text-md"), "glass-text", PANGO_WEIGHT_MEDIUM);
}

static void draw_preview(CapturesWorkbench *self, GtkSnapshot *snapshot, int width, int height) {
  (void)height;
  float x = self->options.floating ? (width - 360) / 2.f : 410, y = self->options.floating ? 60 : 118;
  graphene_rect_t toolbar = GRAPHENE_RECT_INIT(x - 20, y - 46, 324, 38);
  draw_glass(self, snapshot, toolbar);
  text(self, snapshot, self->preview_hidden ? "Saved · preview hidden" : "Mini preview · hover controls", toolbar.origin.x + 14, toolbar.origin.y + 10, number(self, "text-sm"), "glass-text", PANGO_WEIGHT_MEDIUM);
  if (!self->preview_hidden) {
    gtk_snapshot_save(snapshot);
    gtk_snapshot_rotate(snapshot, -3);
    border(snapshot, GRAPHENE_RECT_INIT(x + 16, y + 18, 284, 160), number(self, "r-lg"), rgba(self, "glass-raised"), rgba(self, "glass-border"));
    gtk_snapshot_restore(snapshot);
    border(snapshot, GRAPHENE_RECT_INIT(x, y, 284, 160), number(self, "r-lg"), rgba(self, "glass-raised"), rgba(self, "glass-border-strong"));
    gtk_snapshot_append_texture(snapshot, self->texture, &GRAPHENE_RECT_INIT(x, y, 284, 160));
    graphene_rect_t controls = GRAPHENE_RECT_INIT(x + 52, y + 112, 180, 36);
    rounded(snapshot, controls, 18, rgba(self, "glass-strong"));
    text(self, snapshot, "Copy     Save     Close", controls.origin.x + 15, controls.origin.y + 10, number(self, "text-sm"), "glass-text", PANGO_WEIGHT_MEDIUM);
  } else {
    graphene_rect_t saved = GRAPHENE_RECT_INIT(x + 35, y + 35, 214, 76);
    border(snapshot, saved, number(self, "r-xl"), rgba(self, "glass-strong"), rgba(self, "positive"));
    text(self, snapshot, "✓  Capture saved", saved.origin.x + 28, saved.origin.y + 25, number(self, "text-lg"), "glass-text", PANGO_WEIGHT_SEMIBOLD);
  }
  text(self, snapshot, "Transparent toplevel probe · synthetic only", x - 12, y + 202, number(self, "text-sm"), self->options.floating ? "glass-text-muted" : "text-muted", PANGO_WEIGHT_NORMAL);
}

static void draw_editor(CapturesWorkbench *self, GtkSnapshot *snapshot, int width, int height) {
  char status[128];
  g_snprintf(status, sizeof(status), "2048 × 1152 synthetic image · %.0f%% · %.0f° · drag/scroll exercise", self->zoom * 100, self->rotation);
  text(self, snapshot, status, 220, 52, number(self, "text-sm"), "text-muted", PANGO_WEIGHT_NORMAL);
  graphene_rect_t viewport = GRAPHENE_RECT_INIT(220, 142, width - 248, height - 166);
  rounded(snapshot, viewport, number(self, "r-lg"), rgba(self, "surface-sunken"));
  gtk_snapshot_push_clip(snapshot, &viewport);
  gtk_snapshot_save(snapshot);
  float cx = viewport.origin.x + viewport.size.width / 2 + self->pan_x;
  float cy = viewport.origin.y + viewport.size.height / 2 + self->pan_y;
  gtk_snapshot_translate(snapshot, &GRAPHENE_POINT_INIT(cx, cy));
  gtk_snapshot_rotate(snapshot, self->rotation);
  gtk_snapshot_scale(snapshot, self->zoom, self->zoom);
  gtk_snapshot_append_texture(snapshot, self->texture, &GRAPHENE_RECT_INIT(-284, -160, 568, 320));
  GdkRGBA accent = rgba(self, "theme-accent");
  GskRoundedRect outline;
  gsk_rounded_rect_init_from_rect(&outline, &GRAPHENE_RECT_INIT(-210, -98, 180, 76), number(self, "r-md"));
  float widths[4] = { 3, 3, 3, 3 }; GdkRGBA colors[4] = { accent, accent, accent, accent };
  gtk_snapshot_append_border(snapshot, &outline, widths, colors);
  gtk_snapshot_restore(snapshot);
  gtk_snapshot_pop(snapshot);
  const char *annotation = gtk_editable_get_text(GTK_EDITABLE(self->annotation));
  text(self, snapshot, annotation, cx - 120, cy - 12, number(self, "text-2xl") * self->zoom, "glass-text", PANGO_WEIGHT_BOLD);
}

static void captures_workbench_snapshot(GtkWidget *widget, GtkSnapshot *snapshot) {
  CapturesWorkbench *self = (CapturesWorkbench *)widget;
  self->snapshots++;
  int width = gtk_widget_get_width(widget), height = gtk_widget_get_height(widget);
  draw_chrome(self, snapshot, width, height);
  switch (self->options.scene) {
    case SCENE_PREFERENCES: draw_preferences(self, snapshot); break;
    case SCENE_HISTORY: draw_history(self, snapshot, height); break;
    case SCENE_HUD: draw_hud(self, snapshot, width, height); break;
    case SCENE_PREVIEW: draw_preview(self, snapshot, width, height); break;
    case SCENE_EDITOR: draw_editor(self, snapshot, width, height); break;
    case SCENE_IDLE: break;
  }
  if (self->search && gtk_widget_get_visible(self->search)) gtk_widget_snapshot_child(widget, self->search, snapshot);
  if (self->annotation && gtk_widget_get_visible(self->annotation)) gtk_widget_snapshot_child(widget, self->annotation, snapshot);
}

static void captures_workbench_measure(GtkWidget *widget, GtkOrientation orientation, int for_size,
                                       int *minimum, int *natural, int *minimum_baseline, int *natural_baseline) {
  (void)widget; (void)for_size;
  *minimum = *natural = orientation == GTK_ORIENTATION_HORIZONTAL ? 640 : 480;
  *minimum_baseline = *natural_baseline = -1;
}

static void allocate_child(GtkWidget *child, int width, int height, float x, float y) {
  GskTransform *transform = gsk_transform_translate(NULL, &GRAPHENE_POINT_INIT(x, y));
  gtk_widget_allocate(child, width, height, -1, transform);
}

static void captures_workbench_size_allocate(GtkWidget *widget, int width, int height, int baseline) {
  (void)height; (void)baseline;
  CapturesWorkbench *self = (CapturesWorkbench *)widget;
  if (self->search && gtk_widget_get_visible(self->search)) allocate_child(self->search, width - 248, 40, 220, 86);
  if (self->annotation && gtk_widget_get_visible(self->annotation)) allocate_child(self->annotation, width - 248, 40, 220, 86);
}

static void captures_workbench_dispose(GObject *object) {
  CapturesWorkbench *self = (CapturesWorkbench *)object;
  if (self->animation_timeout) { g_source_remove(self->animation_timeout); self->animation_timeout = 0; }
  if (self->search) { gtk_widget_unparent(self->search); self->search = NULL; }
  if (self->annotation) { gtk_widget_unparent(self->annotation); self->annotation = NULL; }
  g_clear_object(&self->texture);
  if (self->tokens.colors) g_hash_table_destroy(self->tokens.colors);
  if (self->tokens.numbers) g_hash_table_destroy(self->tokens.numbers);
  G_OBJECT_CLASS(captures_workbench_parent_class)->dispose(object);
}

static void captures_workbench_class_init(CapturesWorkbenchClass *klass) {
  GtkWidgetClass *widget = GTK_WIDGET_CLASS(klass);
  widget->snapshot = captures_workbench_snapshot;
  widget->measure = captures_workbench_measure;
  widget->size_allocate = captures_workbench_size_allocate;
  gtk_widget_class_set_accessible_role(widget, GTK_ACCESSIBLE_ROLE_GROUP);
  G_OBJECT_CLASS(klass)->dispose = captures_workbench_dispose;
}

static void captures_workbench_init(CapturesWorkbench *self) {
  gtk_widget_set_focusable(GTK_WIDGET(self), TRUE);
  self->zoom = 1;
  self->selected_row = G_MAXUINT;
  self->texture = fixture_texture();
}

static void editable_changed(GtkEditable *editable, gpointer data) {
  CapturesWorkbench *self = data;
  GtkWidget *widget = GTK_WIDGET(editable);
  GtkWidget *focus = gtk_root_get_focus(gtk_widget_get_root(widget));
  gboolean focused = focus && (focus == widget || gtk_widget_is_ancestor(focus, widget));
  emit("text-changed", "s:value", gtk_editable_get_text(editable), "b:focused", focused, NULL);
  gtk_widget_queue_draw(GTK_WIDGET(self));
}

static gboolean scroll_event(GtkEventControllerScroll *controller, double dx, double dy, gpointer data) {
  (void)controller; (void)dx;
  CapturesWorkbench *self = data;
  if (self->options.scene == SCENE_HISTORY) {
    double max = MAX(0, self->options.history_count * 74.0 - 530);
    self->scroll = CLAMP(self->scroll + dy * 48, 0, max);
  } else if (self->options.scene == SCENE_EDITOR) self->zoom = CLAMP(self->zoom * exp(-dy * .08), .25, 3);
  gtk_widget_queue_draw(GTK_WIDGET(self));
  return TRUE;
}

static void click_pressed(GtkGestureClick *gesture, int count, double x, double y, gpointer data) {
  (void)gesture; (void)count;
  CapturesWorkbench *self = data;
  if (self->options.scene == SCENE_HISTORY && x >= 220 && y >= 122) {
    self->selected_row = MIN(self->options.history_count - 1, (guint)((y - 126 + self->scroll) / 74));
    emit("history-selection", "i:row", (int)self->selected_row, NULL);
  } else if (self->options.scene == SCENE_HUD) self->paused = !self->paused;
  else if (self->options.scene == SCENE_PREVIEW) self->preview_hidden = !self->preview_hidden;
  gtk_widget_queue_draw(GTK_WIDGET(self));
}

static gboolean animation_frame(GtkWidget *widget, GdkFrameClock *clock, gpointer data) {
  (void)clock; (void)data;
  gtk_widget_queue_draw(widget);
  return G_SOURCE_CONTINUE;
}

static gboolean settle_animation(gpointer data) {
  CapturesWorkbench *self = data;
  self->animation_timeout = 0;
  if (self->animation_tick) {
    gtk_widget_remove_tick_callback(GTK_WIDGET(self), self->animation_tick);
    self->animation_tick = 0;
  }
  self->preview_hidden = TRUE;
  gtk_widget_queue_draw(GTK_WIDGET(self));
  emit("animation-settled", "d:seconds", (g_get_monotonic_time() - self->animation_started) / 1000000.0, NULL);
  return G_SOURCE_REMOVE;
}

static gboolean exercise_action(gpointer data) {
  CapturesWorkbench *self = data;
  guint cycle = self->actions++;
  switch (self->options.scene) {
    case SCENE_PREFERENCES:
      gtk_widget_grab_focus(self->search);
      gtk_editable_set_text(GTK_EDITABLE(self->search), cycle % 2 ? "capture" : "appearance");
      break;
    case SCENE_HISTORY:
      self->scroll = cycle % 2 ? 0 : MAX(0, self->options.history_count * 74.0 - 530);
      self->selected_row = cycle % 2 ? 0 : self->options.history_count - 1;
      break;
    case SCENE_HUD: self->paused = !self->paused; self->muted = cycle % 3 == 0; break;
    case SCENE_PREVIEW:
      self->preview_hidden = FALSE;
      self->animation_started = g_get_monotonic_time();
      if (self->animation_tick) gtk_widget_remove_tick_callback(GTK_WIDGET(self), self->animation_tick);
      if (self->animation_timeout) g_source_remove(self->animation_timeout);
      if (self->options.reduced_motion) {
        self->preview_hidden = TRUE;
        self->animation_tick = 0;
        self->animation_timeout = 0;
        emit("animation-settled", "d:seconds", 0.0, NULL);
      } else {
        self->animation_tick = gtk_widget_add_tick_callback(GTK_WIDGET(self), animation_frame, self, NULL);
        self->animation_timeout = g_timeout_add(700, settle_animation, self);
      }
      break;
    case SCENE_EDITOR:
      gtk_widget_grab_focus(self->annotation);
      self->zoom = cycle % 2 ? .75 : 1.5;
      self->rotation = cycle * 15;
      gtk_editable_set_text(GTK_EDITABLE(self->annotation), cycle % 2 ? "Editable GTK text" : "A capture worth keeping");
      break;
    case SCENE_IDLE: return G_SOURCE_REMOVE;
  }
  emit("scripted-action", "s:scene", scene_name(self->options.scene), "i:cycle", (int)cycle,
       "d:zoom", self->zoom, "d:rotation", self->rotation, "b:paused", self->paused,
       "b:previewHidden", self->preview_hidden, "i:selectedRow", self->selected_row == G_MAXUINT ? -1 : (int)self->selected_row,
       "d:scroll", self->scroll,
       "d:elapsedSeconds", (g_get_monotonic_time() - self->started) / 1000000.0, NULL);
  gtk_widget_queue_draw(GTK_WIDGET(self));
  if (self->actions < 6) g_timeout_add(4000, exercise_action, self);
  return G_SOURCE_REMOVE;
}

static Tokens load_tokens(const Options *options) {
  Tokens tokens = { g_hash_table_new_full(g_str_hash, g_str_equal, g_free, g_free),
                    g_hash_table_new_full(g_str_hash, g_str_equal, g_free, g_free) };
  g_autoptr(JsonParser) parser = json_parser_new();
  const char *resource = g_getenv("CAPTURES_GTK_TOKENS");
  if (!resource) resource = "apps/native/gtk/resources/tokens.json";
  g_autoptr(GError) error = NULL;
  if (!json_parser_load_from_file(parser, resource, &error)) g_error("Cannot load %s: %s", resource, error->message);
  JsonObject *root = json_node_get_object(json_parser_get_root(parser));
  g_autofree char *variant = g_strdup_printf("%s-%s", !strcmp(options->appearance, "light") ? "light" : "dark", options->theme);
  JsonObject *object = json_object_get_object_member(root, variant);
  JsonObject *colors = json_object_get_object_member(object, "colors");
  GList *members = json_object_get_members(colors);
  for (GList *item = members; item; item = item->next) {
    JsonArray *array = json_object_get_array_member(colors, item->data);
    GdkRGBA *value = g_new(GdkRGBA, 1);
    value->red = json_array_get_double_element(array, 0); value->green = json_array_get_double_element(array, 1);
    value->blue = json_array_get_double_element(array, 2); value->alpha = json_array_get_double_element(array, 3);
    g_hash_table_insert(tokens.colors, g_strdup(item->data), value);
  }
  g_list_free(members);
  JsonObject *numbers = json_object_get_object_member(object, "numbers");
  members = json_object_get_members(numbers);
  for (GList *item = members; item; item = item->next) {
    double *value = g_new(double, 1); *value = json_object_get_double_member(numbers, item->data);
    g_hash_table_insert(tokens.numbers, g_strdup(item->data), value);
  }
  g_list_free(members);
  return tokens;
}

static CapturesWorkbench *workbench_new(const Options *options) {
  CapturesWorkbench *self = g_object_new(captures_workbench_get_type(), NULL);
  self->options = *options;
  self->tokens = load_tokens(options);
  self->started = g_get_monotonic_time();
  self->search = gtk_entry_new();
  gtk_entry_set_placeholder_text(GTK_ENTRY(self->search), "Find a fixture setting…");
  gtk_accessible_update_property(GTK_ACCESSIBLE(self->search), GTK_ACCESSIBLE_PROPERTY_LABEL, "Search preferences", -1);
  gtk_widget_add_css_class(self->search, "captures-entry");
  gtk_widget_set_parent(self->search, GTK_WIDGET(self));
  gtk_widget_set_visible(self->search, options->scene == SCENE_PREFERENCES);
  g_signal_connect(self->search, "changed", G_CALLBACK(editable_changed), self);
  self->annotation = gtk_entry_new();
  gtk_editable_set_text(GTK_EDITABLE(self->annotation), "A capture worth keeping");
  gtk_accessible_update_property(GTK_ACCESSIBLE(self->annotation), GTK_ACCESSIBLE_PROPERTY_LABEL, "Editable canvas text", -1);
  gtk_widget_add_css_class(self->annotation, "captures-entry");
  gtk_widget_set_parent(self->annotation, GTK_WIDGET(self));
  gtk_widget_set_visible(self->annotation, options->scene == SCENE_EDITOR);
  g_signal_connect(self->annotation, "changed", G_CALLBACK(editable_changed), self);
  GtkEventController *scroll = gtk_event_controller_scroll_new(GTK_EVENT_CONTROLLER_SCROLL_VERTICAL);
  g_signal_connect(scroll, "scroll", G_CALLBACK(scroll_event), self);
  gtk_widget_add_controller(GTK_WIDGET(self), scroll);
  GtkGesture *click = gtk_gesture_click_new();
  g_signal_connect(click, "pressed", G_CALLBACK(click_pressed), self);
  gtk_widget_add_controller(GTK_WIDGET(self), GTK_EVENT_CONTROLLER(click));
  return self;
}

static char *css_color(CapturesWorkbench *self, const char *name) {
  GdkRGBA value = rgba(self, name);
  return gdk_rgba_to_string(&value);
}

static void install_css(CapturesWorkbench *self) {
  g_autofree char *field = css_color(self, "surface-field");
  g_autofree char *text_color = css_color(self, "text");
  g_autofree char *border_color = css_color(self, "control-border");
  g_autofree char *focus = css_color(self, "theme-accent");
  g_autofree char *css_text = g_strdup_printf(
    "window { background: transparent; } "
    ".captures-entry { min-height: 38px; padding: 0 12px; border-radius: 8px; "
    "border: 1px solid %s; background: %s; color: %s; caret-color: %s; box-shadow: none; } "
    ".captures-entry:focus, .captures-entry:focus-within { border-color: %s; outline: none; "
    "box-shadow: 0 0 0 2px %s; } .captures-entry text:focus { outline: none; }",
    border_color, field, text_color, focus, focus, focus);
  GtkCssProvider *css = gtk_css_provider_new();
  G_GNUC_BEGIN_IGNORE_DEPRECATIONS
  gtk_css_provider_load_from_data(css, css_text, -1);
  G_GNUC_END_IGNORE_DEPRECATIONS
  gtk_style_context_add_provider_for_display(gdk_display_get_default(), GTK_STYLE_PROVIDER(css),
                                              GTK_STYLE_PROVIDER_PRIORITY_APPLICATION);
  g_object_unref(css);
  emit("probe-contract", "s:selectedAppearance", self->options.appearance,
       "s:entrySurface", field, "s:entryText", text_color, "s:entryFocus", focus,
       "i:textureWidth", 2048, "i:textureHeight", 1152,
       "s:exerciseScheduleSeconds", "2,6,10,14,18,22", NULL);
}

static gboolean save_screenshot(gpointer data) {
  CapturesWorkbench *self = data;
  self->screenshot_source = 0;
  GtkNative *native = gtk_widget_get_native(GTK_WIDGET(self));
  GtkSnapshot *snapshot = gtk_snapshot_new();
  captures_workbench_snapshot(GTK_WIDGET(self), snapshot);
  GskRenderNode *node = gtk_snapshot_free_to_node(snapshot);
  if (!node) g_error("Screenshot produced no render node");
  graphene_rect_t viewport = GRAPHENE_RECT_INIT(0, 0, gtk_widget_get_width(GTK_WIDGET(self)), gtk_widget_get_height(GTK_WIDGET(self)));
  GdkTexture *texture = gsk_renderer_render_texture(gtk_native_get_renderer(native), node, &viewport);
  if (!gdk_texture_save_to_png(texture, self->options.screenshot)) g_error("Cannot save screenshot %s", self->options.screenshot);
  gsize stride = (gsize)gdk_texture_get_width(texture) * 4;
  g_autofree guchar *pixels = g_malloc(stride * gdk_texture_get_height(texture));
  gdk_texture_download(texture, pixels, stride);
  emit("screenshot-saved", "s:path", self->options.screenshot,
       "i:width", gdk_texture_get_width(texture), "i:height", gdk_texture_get_height(texture),
       "i:cornerAlpha", pixels[3], NULL);
  g_object_unref(texture); gsk_render_node_unref(node);
  gtk_window_destroy(GTK_WINDOW(gtk_widget_get_root(GTK_WIDGET(self))));
  return G_SOURCE_REMOVE;
}

static gboolean quit_app(gpointer data) {
  CapturesWorkbench *self = data;
  GtkWindow *window = GTK_WINDOW(gtk_widget_get_root(GTK_WIDGET(self)));
  gboolean visible = gtk_widget_get_visible(GTK_WIDGET(window));
  gboolean mapped = gtk_widget_get_mapped(GTK_WIDGET(window));
  emit("lifecycle-check", "s:scene", scene_name(self->options.scene), "b:nativeVisible", visible, "b:nativeMapped", mapped, NULL);
  gtk_window_destroy(window);
  return G_SOURCE_REMOVE;
}

static void window_destroyed(GtkWidget *widget, gpointer data) {
  (void)widget;
  CapturesWorkbench *self = data;
  emit("exit", "i:snapshotPasses", (int)self->snapshots, "i:scriptedActions", (int)self->actions,
       "d:elapsedSeconds", (g_get_monotonic_time() - self->started) / 1000000.0,
       "s:note", "GTK snapshot callbacks only; not compositor presentation FPS", NULL);
}

static void activate(GtkApplication *application, gpointer data) {
  Options *options = data;
  GtkWindow *window = GTK_WINDOW(gtk_application_window_new(application));
  gtk_window_set_title(window, "Captures GTK4 comparison workbench");
  gtk_window_set_default_size(window, options->floating ? 640 : 1000, options->floating ? 620 : 720);
  gtk_window_set_decorated(window, !options->floating);
  gtk_widget_add_css_class(GTK_WIDGET(window), options->floating ? "transparent-window" : "captures-window");
  CapturesWorkbench *self = workbench_new(options);
  install_css(self);
  gtk_window_set_child(window, GTK_WIDGET(self));
  g_signal_connect(window, "destroy", G_CALLBACK(window_destroyed), self);
  if (options->scene != SCENE_IDLE) gtk_window_present(window);
  else gtk_widget_set_visible(GTK_WIDGET(window), FALSE);
  const char *backend = G_OBJECT_TYPE_NAME(gdk_display_get_default());
  emit("ready", "s:scene", scene_name(options->scene), "s:renderer", "gtk4-custom-snapshot",
       "s:backend", backend, "s:readiness", "native surface created; not first compositor presentation",
       "b:floating", options->floating, "s:overlayPlacement", strstr(backend, "Wayland") ? "compositor-controlled; absolute placement unsupported" : "window-manager-controlled; no GTK4 absolute positioning API", NULL);
  emit("declared-accessibility", "s:rootRole", "group", "s:searchRole", "text-box", "s:annotationRole", "text-box",
       "s:acceptance", "role declarations only; not AT-SPI verification", NULL);
  if (options->exercise) g_timeout_add(2000, exercise_action, self);
  if (options->screenshot) self->screenshot_source = g_timeout_add((guint)(options->screenshot_after * 1000), save_screenshot, self);
  if (options->quit_after > 0) g_timeout_add((guint)(options->quit_after * 1000), quit_app, self);
}

static Options parse_options(int *argc, char ***argv) {
  Options options = { SCENE_PREFERENCES, "dark", "mustard", 1000, FALSE, FALSE, FALSE, 0, 1, NULL };
  GOptionEntry entries[] = {
    { "appearance", 0, 0, G_OPTION_ARG_STRING, &options.appearance, "light|dark|system", NULL },
    { "theme", 0, 0, G_OPTION_ARG_STRING, &options.theme, "token theme", NULL },
    { "history-count", 0, 0, G_OPTION_ARG_INT, &options.history_count, "0..10000", NULL },
    { "exercise", 0, 0, G_OPTION_ARG_NONE, &options.exercise, "run six actions", NULL },
    { "floating", 0, 0, G_OPTION_ARG_NONE, &options.floating, "transparent HUD/preview toplevel", NULL },
    { "reduced-motion", 0, 0, G_OPTION_ARG_NONE, &options.reduced_motion, "settle immediately", NULL },
    { "quit-after", 0, 0, G_OPTION_ARG_DOUBLE, &options.quit_after, "seconds", NULL },
    { "screenshot", 0, 0, G_OPTION_ARG_FILENAME, &options.screenshot, "PNG path", NULL },
    { "screenshot-after", 0, 0, G_OPTION_ARG_DOUBLE, &options.screenshot_after, "seconds", NULL },
    { NULL }
  };
  const char *scene = "preferences";
  GOptionEntry scene_entry[] = { { "scene", 0, 0, G_OPTION_ARG_STRING, &scene, "preferences|history|hud|preview|editor|idle", NULL }, { NULL } };
  g_autoptr(GOptionContext) context = g_option_context_new("— Captures GTK4 fixture workbench");
  g_option_context_add_main_entries(context, entries, NULL);
  g_option_context_add_main_entries(context, scene_entry, NULL);
  g_autoptr(GError) error = NULL;
  if (!g_option_context_parse(context, argc, argv, &error)) g_error("%s", error->message);
  const char *names[] = { "preferences", "history", "hud", "preview", "editor", "idle" };
  gboolean found = FALSE;
  for (int i = 0; i < 6; i++) if (!strcmp(scene, names[i])) { options.scene = (Scene)i; found = TRUE; }
  const char *themes[] = { "mustard", "ember", "rose", "violet", "cobalt", "aqua", "mint", "lime", "mono" };
  gboolean theme_found = FALSE;
  for (int i = 0; i < 9; i++) if (!strcmp(options.theme, themes[i])) theme_found = TRUE;
  if (!found || !theme_found || (strcmp(options.appearance, "light") && strcmp(options.appearance, "dark")) || options.history_count > 10000)
    g_error("Invalid scene, appearance, theme, or history count");
  if (options.floating && options.scene != SCENE_HUD && options.scene != SCENE_PREVIEW) g_error("Floating is only valid for HUD/preview");
  if (options.scene == SCENE_IDLE && (options.exercise || options.screenshot)) g_error("Hidden idle cannot exercise or screenshot");
  return options;
}

int main(int argc, char **argv) {
  Options options = parse_options(&argc, &argv);
  gtk_init();
  GtkApplication *application = gtk_application_new("com.captures.gtk-workbench", G_APPLICATION_NON_UNIQUE);
  g_signal_connect(application, "activate", G_CALLBACK(activate), &options);
  int status = g_application_run(G_APPLICATION(application), argc, argv);
  g_object_unref(application);
  return status;
}
