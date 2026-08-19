/** Fade when the pointer is this close to the chip, including the painted edge. */
export const GUIDANCE_APPROACH_PAD_PX = 28;
/**
 * Extra slack (px) beyond the approach pad before a faded chip restores.
 * Larger leave zone than enter zone prevents edge thrash while the pointer
 * rests on the border during the opacity transition.
 */
export const GUIDANCE_LEAVE_SLACK_PX = 12;

/** Hit-test for capture guidance with enter/leave hysteresis. */
export function isPointerOverCaptureGuidance(
  clientX: number,
  clientY: number,
  bounds: Pick<DOMRect, "left" | "right" | "top" | "bottom">,
  currentlyOver: boolean,
  options?: { approachPad?: number; leaveSlack?: number },
): boolean {
  const approachPad = options?.approachPad ?? GUIDANCE_APPROACH_PAD_PX;
  const leaveSlack = options?.leaveSlack ?? GUIDANCE_LEAVE_SLACK_PX;
  const pad = currentlyOver ? approachPad + leaveSlack : approachPad;
  return (
    clientX >= bounds.left - pad
    && clientX <= bounds.right + pad
    && clientY >= bounds.top - pad
    && clientY <= bounds.bottom + pad
  );
}
