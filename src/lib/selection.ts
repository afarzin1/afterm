/**
 * `MouseEvent.button` for the left button, the only one that selects. The
 * right button pastes and the middle one is xterm's, and neither starts a
 * drag this module has anything to say about.
 */
export const PRIMARY_BUTTON = 0;

/**
 * Whether copy-on-select is on, read from what the setting was last stored as.
 *
 * On unless it has been turned off, which is why this is not a plain
 * comparison against `"true"`. Three states arrive here and only two of them
 * are the same: `"true"` and `"false"` are answers the user gave, and `null`
 * is a machine that has never opened the settings panel. That last one takes
 * the default, so the feature is on for everyone who has not said otherwise
 * while someone who turned it off keeps it off across a restart.
 */
export function copiesOnSelect(stored: string | null): boolean {
  return stored !== "false";
}

/**
 * Whether letting go of the mouse should put the selection on the clipboard.
 *
 * Copy-on-select is on by default and this is the whole of the rule. Three
 * things have to hold, and each one is a way the feature goes wrong if it is
 * left out.
 *
 * The setting has to be on, because a drag that writes to the clipboard has
 * spent whatever was on it.
 *
 * The drag has to have started in the pane doing the copying. A release is
 * heard on the window — a drag that begins in a terminal can end over another
 * pane, over the sidebar or past the edge of the window, and those are
 * ordinary drags rather than cancelled ones — so without this a click
 * anywhere in the interface would copy a selection some pane was still
 * holding from minutes ago, over a clipboard the user had filled since.
 *
 * And something has to be selected, so that the click which clears a
 * selection is not followed by a write.
 *
 * Kept out of the pane and free of the DOM so the rule can be read and tested
 * on its own, the way `typingIntoField` is.
 */
export function copiesOnRelease(
  enabled: boolean, draggedFromHere: boolean, selected: boolean
): boolean {
  return enabled && draggedFromHere && selected;
}
