package dev.flare.agent;

import android.accessibilityservice.AccessibilityServiceInfo;
import android.app.UiAutomation;
import android.net.Credentials;
import android.net.LocalServerSocket;
import android.net.LocalSocket;
import android.os.Build;
import android.os.HandlerThread;
import android.os.Looper;
import android.view.accessibility.AccessibilityEvent;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileInputStream;
import java.io.InputStream;
import java.io.InputStreamReader;
import java.io.OutputStream;
import java.lang.reflect.Constructor;
import java.nio.charset.StandardCharsets;
import java.security.MessageDigest;
import java.util.ArrayList;
import java.util.List;
import org.json.JSONObject;

/**
 * Flare's agent helper (Android plan PR A3, rule 1369). Started by the emu host with app_process as Android's shell
 * user, inside an emulator only. It holds one UiAutomation connection and answers JSON lines on an abstract socket,
 * which the emu host reaches through `adb forward tcp:0 localabstract:flare-agent`.
 *
 * Requests: {"id":1,"op":"hello"|"tree"|"set_text"|"front_app"|"quit", ...}. Answers: {"id":1,"ok":{...}} or
 * {"id":1,"err":"why","code":"unavailable"|"not_found"|"busy"}. Events (no id): {"event":"windows","count":n}.
 *
 * It ends by itself when no client has been connected for a while, so it never outlives Flare by much, and it frees
 * the UiAutomation connection for a UI test or `uiautomator dump`.
 */
public final class Agent {
    /** The wire protocol's version. Raise it when a request or answer changes shape. */
    static final int PROTOCOL = 1;
    /** The abstract socket's name. One helper per device; a second Flare reuses it. */
    static final String SOCKET = "flare-agent";
    /** How long the helper waits for its first client, and lives on after its last one left. */
    static final long IDLE_MS = 15_000;
    /** At most this many window events a second go to the clients (they are counted, not lost). */
    static final long EVENT_GAP_MS = 100;

    static UiAutomation ui;
    static final Object uiLock = new Object();
    static String jarSha = "";
    static final List<Client> clients = new ArrayList<>();
    static long lastClientGone = System.currentTimeMillis();
    static long windowEvents = 0;
    static long eventsSent = 0;
    static volatile boolean quitting = false;

    public static void main(String[] args) throws Exception {
        Looper.prepareMainLooper();
        String socket = args.length > 0 ? args[0] : SOCKET;
        jarSha = ownSha();
        LocalServerSocket server;
        try {
            server = new LocalServerSocket(socket);
        } catch (Exception e) {
            // Another helper holds the name: the emu host stops it first, so this only happens in a race.
            System.out.println("taken " + socket);
            System.out.flush();
            System.exit(3);
            return;
        }
        HandlerThread ht = new HandlerThread("flare-agent-ui");
        ht.start();
        try {
            ui = connect(ht.getLooper());
        } catch (Throwable t) {
            System.out.println("no_ui " + oneLine(t));
            System.out.flush();
            System.exit(4);
            return;
        }
        // No lambdas anywhere: android.jar has no LambdaMetafactory for javac to link against.
        ui.setOnAccessibilityEventListener(new UiAutomation.OnAccessibilityEventListener() {
            @Override
            public void onAccessibilityEvent(AccessibilityEvent e) {
                synchronized (clients) {
                    windowEvents++;
                }
            }
        });
        startWatchdog();
        System.out.println("ready " + PROTOCOL + " " + jarSha + " " + socket);
        System.out.flush();
        while (true) {
            LocalSocket s = server.accept();
            Credentials c = s.getPeerCredentials();
            // Rule 1369: only adb's forward (adbd runs as shell, or root after `adb root`) may talk to the helper. An
            // app inside the device is refused.
            if (c == null || (c.getUid() != 2000 && c.getUid() != 0)) {
                s.close();
                continue;
            }
            final Client cl = new Client(s);
            synchronized (clients) {
                clients.add(cl);
            }
            Thread t = new Thread(new Runnable() {
                @Override
                public void run() {
                    cl.serve();
                }
            }, "flare-agent-client");
            t.setDaemon(true);
            t.start();
        }
    }

    /** UiAutomation over the shell's UiAutomationConnection, the way `uiautomator` and `am instrument` make it. */
    static UiAutomation connect(Looper looper) throws Exception {
        Class<?> connCls = Class.forName("android.app.UiAutomationConnection");
        Object conn = connCls.getConstructor().newInstance();
        Class<?> iface = Class.forName("android.app.IUiAutomationConnection");
        Constructor<UiAutomation> c = UiAutomation.class.getDeclaredConstructor(Looper.class, iface);
        c.setAccessible(true);
        UiAutomation u = c.newInstance(looper, conn);
        // Keep the person's own accessibility services (TalkBack and so on) running.
        try {
            UiAutomation.class.getDeclaredMethod("connect", int.class)
                    .invoke(u, UiAutomation.FLAG_DONT_SUPPRESS_ACCESSIBILITY_SERVICES);
        } catch (NoSuchMethodException e) {
            UiAutomation.class.getDeclaredMethod("connect").invoke(u);
        }
        AccessibilityServiceInfo info = u.getServiceInfo();
        info.flags |= AccessibilityServiceInfo.FLAG_RETRIEVE_INTERACTIVE_WINDOWS
                | AccessibilityServiceInfo.FLAG_REPORT_VIEW_IDS
                | AccessibilityServiceInfo.FLAG_INCLUDE_NOT_IMPORTANT_VIEWS;
        info.eventTypes = AccessibilityEvent.TYPE_WINDOWS_CHANGED
                | AccessibilityEvent.TYPE_WINDOW_STATE_CHANGED
                | AccessibilityEvent.TYPE_WINDOW_CONTENT_CHANGED;
        u.setServiceInfo(info);
        return u;
    }

    /** Sends the window-event count at most every EVENT_GAP_MS, and ends the helper when it has been alone too long. */
    static void startWatchdog() {
        Thread t = new Thread(new Runnable() {
            @Override
            public void run() {
                watch();
            }
        }, "flare-agent-watch");
        t.setDaemon(true);
        t.start();
    }

    static void watch() {
        while (true) {
            try {
                Thread.sleep(EVENT_GAP_MS);
            } catch (InterruptedException e) {
                return;
            }
            if (quitting) System.exit(0);
            List<Client> now;
            long count;
            synchronized (clients) {
                if (clients.isEmpty()) {
                    if (System.currentTimeMillis() - lastClientGone > IDLE_MS) {
                        System.exit(0);
                    }
                    continue;
                }
                if (windowEvents == eventsSent) continue;
                count = windowEvents;
                eventsSent = count;
                now = new ArrayList<>(clients);
            }
            String line = "{\"event\":\"windows\",\"count\":" + count + "}";
            for (Client c : now) c.send(line);
        }
    }

    /** One connection: requests in, answers and events out, one line each. */
    static final class Client {
        final LocalSocket s;
        OutputStream out;

        Client(LocalSocket s) {
            this.s = s;
        }

        void serve() {
            try {
                out = s.getOutputStream();
                BufferedReader in = new BufferedReader(
                        new InputStreamReader(s.getInputStream(), StandardCharsets.UTF_8));
                String line;
                while ((line = in.readLine()) != null) {
                    if (line.trim().isEmpty()) continue;
                    send(answer(line));
                }
            } catch (Exception e) {
                // The client went.
            } finally {
                try {
                    s.close();
                } catch (Exception e) {
                    // Closed already.
                }
                synchronized (clients) {
                    clients.remove(this);
                    lastClientGone = System.currentTimeMillis();
                }
            }
        }

        synchronized void send(String line) {
            try {
                out.write((line + "\n").getBytes(StandardCharsets.UTF_8));
                out.flush();
            } catch (Exception e) {
                // The reader sees the close.
            }
        }
    }

    /** One request line, one answer line. */
    static String answer(String line) {
        long id = 0;
        try {
            JSONObject req = new JSONObject(line);
            id = req.optLong("id", 0);
            String op = req.optString("op", "");
            if (op.equals("tree")) {
                // Written by hand: a big tree is faster as one string than as JSONObjects.
                String tree;
                synchronized (uiLock) {
                    tree = Tree.read(ui);
                }
                return "{\"id\":" + id + ",\"ok\":" + tree + "}";
            }
            JSONObject ok;
            synchronized (uiLock) {
                switch (op) {
                    case "hello":
                        ok = hello();
                        break;
                    case "set_text":
                        ok = Text.set(ui, req);
                        break;
                    case "front_app":
                        ok = new JSONObject().put("package", Tree.frontApp(ui));
                        break;
                    case "quit":
                        // The watchdog ends the helper on its next tick, after this answer went out.
                        quitting = true;
                        ok = new JSONObject();
                        break;
                    default:
                        return err(id, "unavailable", "no such request: " + op);
                }
            }
            return new JSONObject().put("id", id).put("ok", ok).toString();
        } catch (Text.Refused r) {
            return err(id, r.code, r.getMessage());
        } catch (Throwable t) {
            return err(id, "unavailable", oneLine(t));
        }
    }

    static JSONObject hello() throws Exception {
        long events;
        synchronized (clients) {
            events = windowEvents;
        }
        return new JSONObject()
                .put("agent", "flare-agent")
                .put("protocol", PROTOCOL)
                .put("jar_sha256", jarSha)
                .put("api", Build.VERSION.SDK_INT)
                .put("release", Build.VERSION.RELEASE)
                .put("pid", android.os.Process.myPid())
                .put("events", events);
    }

    static String err(long id, String code, String why) {
        try {
            return new JSONObject().put("id", id).put("code", code).put("err", why).toString();
        } catch (Exception e) {
            return "{\"id\":" + id + ",\"code\":\"unavailable\",\"err\":\"?\"}";
        }
    }

    static String oneLine(Throwable t) {
        String m = t.getClass().getSimpleName() + ": " + t.getMessage();
        m = m.replace('\n', ' ').replace('\r', ' ');
        return m.length() > 300 ? m.substring(0, 300) : m;
    }

    /** The SHA-256 of this helper's own jar (its class path), so the emu host knows which build answers. */
    static String ownSha() {
        String cp = System.getProperty("java.class.path", "");
        if (cp.isEmpty()) cp = System.getenv("CLASSPATH");
        if (cp == null || cp.isEmpty()) return "";
        try (InputStream in = new FileInputStream(new File(cp.split(":")[0]))) {
            MessageDigest d = MessageDigest.getInstance("SHA-256");
            byte[] buf = new byte[16384];
            int n;
            while ((n = in.read(buf)) > 0) d.update(buf, 0, n);
            StringBuilder sb = new StringBuilder();
            for (byte b : d.digest()) sb.append(String.format("%02x", b & 0xff));
            return sb.toString();
        } catch (Exception e) {
            return "";
        }
    }
}
