/** Inset (px) from the painted edge before the chip starts fading under the cursor. */
export const GUIDANCE_ENTER_INSET_PX = 4;
/**
 * Extra slack (px) beyond the chip bounds before a faded chip restores.
 * Larger leave zone than enter zone prevents edge thrash while the pointer
 * rests on the border during the opacity transition.
 */
export const GUIDANCE_LEAVE_SLACK_PX = 20;

/** Hit-test for capture guidance with enter/leave hysteresis. */
export function isPointerOverCaptureGuidance(
  clientX: number,
  clientY: number,
  bounds: Pick<DOMRect, "left" | "right" | "top" | "bottom">,
  currentlyOver: boolean,
  options?: { enterInset?: number; leaveSlack?: number },
): boolean {
  const enterInset = options?.enterInset ?? GUIDANCE_ENTER_INSET_PX;
  const leaveSlack = options?.leaveSlack ?? GUIDANCE_LEAVE_SLACK_PX;
  if (currentlyOver) {
    return (
      clientX >= bounds.left - leaveSlack
      && clientX <= bounds.right + leaveSlack
      && clientY >= bounds.top - leaveSlack
      && clientY <= bounds.bottom + leaveSlack
    );
  }
  const width = bounds.right - bounds.left;
  const height = bounds.bottom - bounds.top;
  const insetX = Math.min(enterInset, Math.max(0, width / 2 - 1));
  const insetY = Math.min(enterInset, Math.max(0, height / 2 - 1));
  return (
    clientX >= bounds.left + insetX
    && clientX <= bounds.right - insetX
    && clientY >= bounds.top + insetY
    && clientY <= bounds.bottom - insetY
  );
}
