#include "my_application.h"

#include <flutter_linux/flutter_linux.h>

#include "flutter/generated_plugin_registrant.h"

struct _MyApplication {
  GtkApplication parent_instance;
  char** dart_entrypoint_arguments;
  GtkWindow* window;
  FlMethodChannel* window_chrome_channel;
};

G_DEFINE_TYPE(MyApplication, my_application, GTK_TYPE_APPLICATION)

static gboolean get_pointer_position(GtkWindow* window, gint* x, gint* y) {
  GdkDisplay* display = gtk_widget_get_display(GTK_WIDGET(window));
  GdkSeat* seat = gdk_display_get_default_seat(display);
  if (seat == nullptr) {
    return FALSE;
  }

  GdkDevice* pointer = gdk_seat_get_pointer(seat);
  if (pointer == nullptr) {
    return FALSE;
  }

  gdk_device_get_position(pointer, nullptr, x, y);
  return TRUE;
}

static GdkWindowEdge resize_edge_from_name(const gchar* name) {
  if (g_strcmp0(name, "north") == 0) return GDK_WINDOW_EDGE_NORTH;
  if (g_strcmp0(name, "south") == 0) return GDK_WINDOW_EDGE_SOUTH;
  if (g_strcmp0(name, "west") == 0) return GDK_WINDOW_EDGE_WEST;
  if (g_strcmp0(name, "east") == 0) return GDK_WINDOW_EDGE_EAST;
  if (g_strcmp0(name, "northWest") == 0) return GDK_WINDOW_EDGE_NORTH_WEST;
  if (g_strcmp0(name, "northEast") == 0) return GDK_WINDOW_EDGE_NORTH_EAST;
  if (g_strcmp0(name, "southWest") == 0) return GDK_WINDOW_EDGE_SOUTH_WEST;
  return GDK_WINDOW_EDGE_SOUTH_EAST;
}

// Handles the borderless window gestures requested by Flutter's custom
// chrome: drag the header to move, or drag any outer edge/corner to resize.
static void window_chrome_method_call_cb(FlMethodChannel* channel,
                                         FlMethodCall* method_call,
                                         gpointer user_data) {
  MyApplication* self = MY_APPLICATION(user_data);
  const gchar* method = fl_method_call_get_name(method_call);
  gint x = 0;
  gint y = 0;

  if (g_strcmp0(method, "beginMove") == 0 &&
      get_pointer_position(self->window, &x, &y)) {
    gtk_window_begin_move_drag(self->window, 1, x, y, GDK_CURRENT_TIME);
  } else if (g_strcmp0(method, "beginResize") == 0 &&
             get_pointer_position(self->window, &x, &y)) {
    FlValue* args = fl_method_call_get_args(method_call);
    if (args != nullptr && fl_value_get_type(args) == FL_VALUE_TYPE_STRING) {
      gtk_window_begin_resize_drag(self->window,
                                   resize_edge_from_name(fl_value_get_string(args)),
                                   1, x, y, GDK_CURRENT_TIME);
    }
  } else if (g_strcmp0(method, "beginMove") != 0 &&
             g_strcmp0(method, "beginResize") != 0) {
    g_autoptr(FlMethodResponse) response =
        FL_METHOD_RESPONSE(fl_method_not_implemented_response_new());
    g_autoptr(GError) error = nullptr;
    fl_method_call_respond(method_call, response, &error);
    return;
  }

  g_autoptr(FlMethodResponse) response =
      FL_METHOD_RESPONSE(fl_method_success_response_new(nullptr));
  g_autoptr(GError) error = nullptr;
  if (!fl_method_call_respond(method_call, response, &error)) {
    g_warning("Failed to respond to window chrome request: %s", error->message);
  }
}

// Called when first Flutter frame received.
static void first_frame_cb(MyApplication* self, FlView* view) {
  gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));
}

// Implements GApplication::activate.
static void my_application_activate(GApplication* application) {
  MyApplication* self = MY_APPLICATION(application);
  GtkWindow* window =
      GTK_WINDOW(gtk_application_window_new(GTK_APPLICATION(application)));
  self->window = window;

  // Use an alpha-capable surface before GTK realizes the window. Combined
  // with Flutter's transparent backing color below, this lets the rounded
  // Dart chrome reveal the desktop at its four corners instead of black.
  GdkScreen* screen = gtk_widget_get_screen(GTK_WIDGET(window));
  GdkVisual* rgba_visual = gdk_screen_get_rgba_visual(screen);
  if (rgba_visual != nullptr) {
    gtk_widget_set_visual(GTK_WIDGET(window), rgba_visual);
    gtk_widget_set_app_paintable(GTK_WIDGET(window), TRUE);
  }

  // AERO>>DSP owns its full window chrome. This removes GNOME's header bar
  // and X11's traditional title bar so Flutter can supply the matching
  // blue-chrome controls inside the app itself.
  gtk_window_set_decorated(window, FALSE);
  gtk_window_set_title(window, "AERO>>DSP");

  gtk_window_set_default_size(window, 1280, 720);

  g_autoptr(FlDartProject) project = fl_dart_project_new();
  fl_dart_project_set_dart_entrypoint_arguments(
      project, self->dart_entrypoint_arguments);

  FlView* view = fl_view_new(project);
  GdkRGBA background_color;
  // The Flutter view must not refill the alpha-capable GTK surface behind the
  // rounded Dart chrome. A transparent view background preserves the desktop
  // at the clipped corners.
  gdk_rgba_parse(&background_color, "#00000000");
  fl_view_set_background_color(view, &background_color);
  gtk_widget_show(GTK_WIDGET(view));
  gtk_container_add(GTK_CONTAINER(window), GTK_WIDGET(view));

  // Show the window when Flutter renders.
  // Requires the view to be realized so we can start rendering.
  g_signal_connect_swapped(view, "first-frame", G_CALLBACK(first_frame_cb),
                           self);
  gtk_widget_realize(GTK_WIDGET(view));

  g_autoptr(FlStandardMethodCodec) codec = fl_standard_method_codec_new();
  self->window_chrome_channel = fl_method_channel_new(
      fl_engine_get_binary_messenger(fl_view_get_engine(view)),
      "aero_dsp/window_chrome", FL_METHOD_CODEC(codec));
  fl_method_channel_set_method_call_handler(self->window_chrome_channel,
                                            window_chrome_method_call_cb, self,
                                            nullptr);

  fl_register_plugins(FL_PLUGIN_REGISTRY(view));

  gtk_widget_grab_focus(GTK_WIDGET(view));
}

// Implements GApplication::local_command_line.
static gboolean my_application_local_command_line(GApplication* application,
                                                  gchar*** arguments,
                                                  int* exit_status) {
  MyApplication* self = MY_APPLICATION(application);
  // Strip out the first argument as it is the binary name.
  self->dart_entrypoint_arguments = g_strdupv(*arguments + 1);

  g_autoptr(GError) error = nullptr;
  if (!g_application_register(application, nullptr, &error)) {
    g_warning("Failed to register: %s", error->message);
    *exit_status = 1;
    return TRUE;
  }

  g_application_activate(application);
  *exit_status = 0;

  return TRUE;
}

// Implements GApplication::startup.
static void my_application_startup(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application startup.

  G_APPLICATION_CLASS(my_application_parent_class)->startup(application);
}

// Implements GApplication::shutdown.
static void my_application_shutdown(GApplication* application) {
  // MyApplication* self = MY_APPLICATION(object);

  // Perform any actions required at application shutdown.

  G_APPLICATION_CLASS(my_application_parent_class)->shutdown(application);
}

// Implements GObject::dispose.
static void my_application_dispose(GObject* object) {
  MyApplication* self = MY_APPLICATION(object);
  g_clear_object(&self->window_chrome_channel);
  g_clear_pointer(&self->dart_entrypoint_arguments, g_strfreev);
  G_OBJECT_CLASS(my_application_parent_class)->dispose(object);
}

static void my_application_class_init(MyApplicationClass* klass) {
  G_APPLICATION_CLASS(klass)->activate = my_application_activate;
  G_APPLICATION_CLASS(klass)->local_command_line =
      my_application_local_command_line;
  G_APPLICATION_CLASS(klass)->startup = my_application_startup;
  G_APPLICATION_CLASS(klass)->shutdown = my_application_shutdown;
  G_OBJECT_CLASS(klass)->dispose = my_application_dispose;
}

static void my_application_init(MyApplication* self) {}

MyApplication* my_application_new() {
  // Set the program name to the application ID, which helps various systems
  // like GTK and desktop environments map this running application to its
  // corresponding .desktop file. This ensures better integration by allowing
  // the application to be recognized beyond its binary name.
  g_set_prgname(APPLICATION_ID);

  return MY_APPLICATION(g_object_new(my_application_get_type(),
                                     "application-id", APPLICATION_ID, "flags",
                                     G_APPLICATION_NON_UNIQUE, nullptr));
}
