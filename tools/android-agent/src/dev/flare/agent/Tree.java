package dev.flare.agent;

import android.app.UiAutomation;
import android.graphics.Rect;
import android.os.Build;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;
import java.util.List;
import org.json.JSONObject;

/**
 * The UI tree: every window from getWindows() (FLAG_RETRIEVE_INTERACTIVE_WINDOWS), each node with what the emu host
 * maps onto the tree model (`flare_device::emu::agent`). Bounds are screen pixels in the current rotation; the emu
 * host turns them into dp. Keys that are false or empty are left out, to keep a big tree small.
 *
 * {"front_app":"pkg","nodes":n,"capped":false,"us":t,"windows":[{"type":1,"layer":0,"active":true,"focused":true,
 *  "package":"pkg","title":"..","b":[l,t,r,b],"root":NODE}]}
 * NODE = {"class":"..","text":"..","desc":"..","hint":"..","res_id":"..","b":[l,t,r,b],"clickable":true,
 *  "long_clickable":true,"scrollable":true,"editable":true,"checkable":true,"checked":true,"focused":true,
 *  "selected":true,"password":true,"showing_hint":true,"disabled":true,"ch":[NODE,...]}
 */
final class Tree {
    /** The most nodes one tree holds, and the deepest it goes (a runaway app cannot make an endless answer). */
    static final int MAX_NODES = 5000;
    static final int MAX_DEPTH = 80;
    /** The longest text kept per field, in UTF-16 units. */
    static final int TEXT_MAX = 2000;

    private Tree() {}

    static String read(UiAutomation ui) {
        long t0 = System.nanoTime();
        int[] count = {0};
        boolean[] capped = {false};
        StringBuilder sb = new StringBuilder(64 * 1024);
        sb.append("{\"windows\":[");
        List<AccessibilityWindowInfo> ws = ui.getWindows();
        boolean first = true;
        for (AccessibilityWindowInfo w : ws) {
            AccessibilityNodeInfo r = w.getRoot();
            if (r == null) continue;
            if (!first) sb.append(',');
            first = false;
            Rect b = new Rect();
            w.getBoundsInScreen(b);
            sb.append("{\"type\":").append(w.getType())
              .append(",\"layer\":").append(w.getLayer());
            if (w.isActive()) sb.append(",\"active\":true");
            if (w.isFocused()) sb.append(",\"focused\":true");
            str(sb, "package", r.getPackageName());
            str(sb, "title", w.getTitle());
            rect(sb, b);
            sb.append(",\"root\":");
            walk(r, sb, count, capped, 0);
            sb.append('}');
            recycle(r);
        }
        sb.append(']');
        str(sb, "front_app", frontApp(ui));
        sb.append(",\"nodes\":").append(count[0]);
        if (capped[0]) sb.append(",\"capped\":true");
        sb.append(",\"us\":").append((System.nanoTime() - t0) / 1000);
        sb.append('}');
        return sb.toString();
    }

    /** The package of the window in front (the active one). Empty when there is none. */
    static String frontApp(UiAutomation ui) {
        AccessibilityNodeInfo r = ui.getRootInActiveWindow();
        if (r == null) return "";
        CharSequence p = r.getPackageName();
        recycle(r);
        return p == null ? "" : p.toString();
    }

    static void walk(AccessibilityNodeInfo n, StringBuilder sb, int[] count, boolean[] capped, int depth) {
        count[0]++;
        Rect r = new Rect();
        n.getBoundsInScreen(r);
        sb.append('{');
        sb.append("\"class\":").append(JSONObject.quote(s(n.getClassName())));
        boolean password = n.isPassword();
        // A password field's text is never read out (the helper sends no secret it can avoid).
        if (!password) str(sb, "text", n.getText());
        str(sb, "desc", n.getContentDescription());
        if (Build.VERSION.SDK_INT >= 26) str(sb, "hint", n.getHintText());
        str(sb, "res_id", n.getViewIdResourceName());
        rect(sb, r);
        flag(sb, "clickable", n.isClickable());
        flag(sb, "long_clickable", n.isLongClickable());
        flag(sb, "scrollable", n.isScrollable());
        flag(sb, "editable", n.isEditable());
        flag(sb, "checkable", n.isCheckable());
        flag(sb, "checked", n.isChecked());
        flag(sb, "focused", n.isFocused());
        flag(sb, "selected", n.isSelected());
        flag(sb, "password", password);
        if (Build.VERSION.SDK_INT >= 26) flag(sb, "showing_hint", n.isShowingHintText());
        flag(sb, "disabled", !n.isEnabled());
        int kids = n.getChildCount();
        if (kids > 0 && depth < MAX_DEPTH) {
            sb.append(",\"ch\":[");
            boolean first = true;
            for (int i = 0; i < kids; i++) {
                if (count[0] >= MAX_NODES) {
                    capped[0] = true;
                    break;
                }
                AccessibilityNodeInfo c = n.getChild(i);
                if (c == null) continue;
                if (!first) sb.append(',');
                first = false;
                walk(c, sb, count, capped, depth + 1);
                recycle(c);
            }
            sb.append(']');
        } else if (kids > 0) {
            capped[0] = true;
        }
        sb.append('}');
    }

    static String s(CharSequence c) {
        if (c == null) return "";
        String v = c.toString();
        return v.length() > TEXT_MAX ? v.substring(0, TEXT_MAX) : v;
    }

    static void str(StringBuilder sb, String key, CharSequence v) {
        String t = s(v);
        if (t.isEmpty()) return;
        sb.append(",\"").append(key).append("\":").append(JSONObject.quote(t));
    }

    static void flag(StringBuilder sb, String key, boolean v) {
        if (v) sb.append(",\"").append(key).append("\":true");
    }

    static void rect(StringBuilder sb, Rect r) {
        sb.append(",\"b\":[").append(r.left).append(',').append(r.top).append(',')
          .append(r.right).append(',').append(r.bottom).append(']');
    }

    @SuppressWarnings("deprecation")
    static void recycle(AccessibilityNodeInfo n) {
        // A no-op from API 33; earlier it returns the node to the pool.
        if (Build.VERSION.SDK_INT < 33) n.recycle();
    }
}
