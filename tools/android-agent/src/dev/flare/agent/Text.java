package dev.flare.agent;

import android.app.UiAutomation;
import android.graphics.Rect;
import android.os.Build;
import android.os.Bundle;
import android.view.accessibility.AccessibilityNodeInfo;
import android.view.accessibility.AccessibilityWindowInfo;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * set_text (rule 1368): {"op":"set_text","text":"...","mode":"insert"|"replace","at":[x,y]}. The field is the one
 * with input focus, or with "at" the deepest editable node under that screen pixel. "insert" (the default) types the
 * text at the cursor, as keys would: the field's text is read, the text goes in place of the selection, and the
 * cursor goes after it. ACTION_SET_TEXT alone would replace the whole field.
 *
 * A password field is refused (its text cannot be read back to splice into); the emu host then pastes through the
 * device's own clipboard.
 */
final class Text {
    static final class Refused extends Exception {
        final String code;

        Refused(String code, String why) {
            super(why);
            this.code = code;
        }
    }

    private Text() {}

    static JSONObject set(UiAutomation ui, JSONObject req) throws Exception {
        String text = req.optString("text", "");
        boolean replace = "replace".equals(req.optString("mode", "insert"));
        JSONArray at = req.optJSONArray("at");
        AccessibilityNodeInfo f = at != null && at.length() == 2
                ? editableAt(ui, at.getInt(0), at.getInt(1))
                : ui.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
        if (f == null) throw new Refused("not_found", "no text field has the focus");
        try {
            if (!f.isEditable()) throw new Refused("not_found", "the focused element does not take text");
            if (f.isPassword()) throw new Refused("unavailable", "a password field");
            String now = current(f);
            String next;
            int cursor;
            if (replace) {
                next = text;
                cursor = text.length();
            } else {
                int a = f.getTextSelectionStart();
                int b = f.getTextSelectionEnd();
                if (a < 0 || b < 0 || a > now.length() || b > now.length()) {
                    a = now.length();
                    b = now.length();
                }
                int lo = Math.min(a, b);
                int hi = Math.max(a, b);
                next = now.substring(0, lo) + text + now.substring(hi);
                cursor = lo + text.length();
            }
            Bundle args = new Bundle();
            args.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, next);
            if (!f.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, args)) {
                throw new Refused("unavailable", "the field did not take the text");
            }
            // The node is stale after the new text: read it again, or the selection is refused.
            f.refresh();
            Bundle sel = new Bundle();
            sel.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, cursor);
            sel.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, cursor);
            boolean placed = f.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, sel);
            f.refresh();
            JSONObject ok = new JSONObject().put("cursor", placed);
            // Said back only when it is what was asked for, so a mismatch is seen without sending the field's text.
            ok.put("matches", next.equals(current(f)));
            return ok;
        } finally {
            Tree.recycle(f);
        }
    }

    /** The field's text, or "" while it shows its hint (some fields give the hint as their text then). */
    static String current(AccessibilityNodeInfo f) {
        if (Build.VERSION.SDK_INT >= 26 && f.isShowingHintText()) return "";
        CharSequence t = f.getText();
        return t == null ? "" : t.toString();
    }

    /** The deepest editable node whose bounds hold the pixel, in the top-most window that has one. */
    static AccessibilityNodeInfo editableAt(UiAutomation ui, int x, int y) {
        for (AccessibilityWindowInfo w : ui.getWindows()) {
            AccessibilityNodeInfo r = w.getRoot();
            if (r == null) continue;
            AccessibilityNodeInfo hit = deepest(r, x, y, 0);
            if (hit != r) Tree.recycle(r);
            if (hit != null) return hit;
        }
        return null;
    }

    static AccessibilityNodeInfo deepest(AccessibilityNodeInfo n, int x, int y, int depth) {
        Rect b = new Rect();
        n.getBoundsInScreen(b);
        if (!b.contains(x, y) || depth > Tree.MAX_DEPTH) return null;
        for (int i = n.getChildCount() - 1; i >= 0; i--) {
            AccessibilityNodeInfo c = n.getChild(i);
            if (c == null) continue;
            AccessibilityNodeInfo hit = deepest(c, x, y, depth + 1);
            if (hit != c) Tree.recycle(c);
            if (hit != null) return hit;
        }
        return n.isEditable() ? n : null;
    }
}
