//! Shipping's notice keyframes (`captures_app::motion`: the launch notice's
//! arrive, the update notice's pop-in and restart exit, the hidden recording
//! controls' lifecycle) played on a notice window's root DirectComposition
//! visual: opacity, `translateY` and `scale` about the window's centre, sampled
//! from the shared pose curve into compositor animations so no frame is
//! repainted while they run. Under reduced motion each pose changes in one
//! step at its keyframe time, as shipping's global 0.01 ms rule.
use captures_app::motion::{Animation, Pose};
use windows::Win32::Graphics::DirectComposition::*;
use windows::core::{Interface, Result};

/// One sample per frame at 60 Hz.
const STEP_MS: f64 = 1000. / 60.;

/// How a pose is sampled over time.
pub enum Curve<'a> {
    /// `pose_at`, from the moment the motion starts.
    Once(&'a Animation),
    /// A lifecycle notice that closes `until_end_ms` after it starts.
    Lifecycle(&'a Animation, f64),
}

impl Curve<'_> {
    fn pose(&self, ms: f64, reduced: bool) -> Pose {
        match *self {
            Curve::Once(animation) => animation.pose_at(ms, reduced),
            Curve::Lifecycle(animation, end) => {
                animation.lifecycle_pose(ms, Some(end - ms), reduced)
            }
        }
    }

    fn total_ms(&self, reduced: bool) -> f64 {
        match *self {
            Curve::Once(animation) => animation.total_ms(reduced),
            Curve::Lifecycle(_, _) if reduced => 0.,
            Curve::Lifecycle(_, end) => end,
        }
    }

    /// Sample times: the delay is held, then every frame to the end.
    fn times(&self, reduced: bool) -> Vec<f64> {
        let total = self.total_ms(reduced);
        let start = match *self {
            Curve::Once(animation) => animation.delay_ms.min(total),
            Curve::Lifecycle(..) => 0.,
        };
        let mut times = vec![0.];
        if !reduced {
            let mut ms = start;
            while ms < total {
                times.push(ms);
                ms += STEP_MS;
            }
        }
        times.push(total);
        times.dedup();
        times
    }
}

/// Play `curve` on `visual`, whose content is `size_px` device pixels and
/// drawn at `scale` (translations are in points).
pub fn play(
    dcomp: &IDCompositionDesktopDevice,
    visual: &IDCompositionVisual2,
    size_px: (f32, f32),
    scale: f32,
    curve: Curve,
    reduced: bool,
) -> Result<()> {
    let times = curve.times(reduced);
    let poses: Vec<Pose> = times.iter().map(|&ms| curve.pose(ms, reduced)).collect();
    let track = |value: &dyn Fn(&Pose) -> f64| -> Result<IDCompositionAnimation> {
        let animation = unsafe { dcomp.CreateAnimation()? };
        for (i, pair) in times.windows(2).enumerate() {
            let (a, b) = (value(&poses[i]), value(&poses[i + 1]));
            let span = (pair[1] - pair[0]) / 1000.;
            // A step (reduced motion, or a held delay) keeps its value until
            // the next sample; frames between samples move linearly.
            let slope = if reduced || span <= 0. {
                0.
            } else {
                (b - a) / span
            };
            unsafe { animation.AddCubic(pair[0] / 1000., a as f32, slope as f32, 0., 0.)? };
        }
        let last = times[times.len() - 1];
        unsafe { animation.End(last / 1000., value(&poses[poses.len() - 1]) as f32)? };
        Ok(animation)
    };
    unsafe {
        let visual3 = visual.cast::<IDCompositionVisual3>()?;
        if times.len() == 1 {
            // Nothing moves: set the pose.
            let pose = poses[0];
            visual3.SetOpacity2(pose.opacity as f32)?;
            let zoom = dcomp.CreateScaleTransform()?;
            zoom.SetCenterX2(size_px.0 / 2.)?;
            zoom.SetCenterY2(size_px.1 / 2.)?;
            zoom.SetScaleX2(pose.scale as f32)?;
            zoom.SetScaleY2(pose.scale as f32)?;
            let shift = dcomp.CreateTranslateTransform()?;
            shift.SetOffsetY2((pose.translate_y * f64::from(scale)) as f32)?;
            let group = dcomp.CreateTransformGroup(&[Some(zoom.cast()?), Some(shift.cast()?)])?;
            return visual.SetTransform(&group);
        }
        let opacity = track(&|p| p.opacity)?;
        visual3.SetOpacity(&opacity)?;
        let zoom = dcomp.CreateScaleTransform()?;
        zoom.SetCenterX2(size_px.0 / 2.)?;
        zoom.SetCenterY2(size_px.1 / 2.)?;
        let factor = track(&|p| p.scale)?;
        zoom.SetScaleX(&factor)?;
        zoom.SetScaleY(&factor)?;
        let shift = dcomp.CreateTranslateTransform()?;
        shift.SetOffsetY(&track(&|p| p.translate_y * f64::from(scale))?)?;
        let group = dcomp.CreateTransformGroup(&[Some(zoom.cast()?), Some(shift.cast()?)])?;
        visual.SetTransform(&group)?;
    }
    Ok(())
}

/// Put `visual` back at rest: fully visible, untransformed.
pub fn rest(dcomp: &IDCompositionDesktopDevice, visual: &IDCompositionVisual2) -> Result<()> {
    unsafe {
        visual.cast::<IDCompositionVisual3>()?.SetOpacity2(1.)?;
        visual.SetTransform2(&windows_numerics::Matrix3x2::identity())?;
        dcomp.Commit()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use captures_app::motion::Motion;

    fn resolve(motion: Motion) -> Animation {
        let tokens = crate::tokens::Tokens::variant("light", "mustard");
        motion.resolve(&tokens).expect("motion")
    }

    #[test]
    fn samples_hold_the_delay_and_reach_the_end() {
        // The restart exit waits 3 s, then fades over `--dur-4`.
        let exit = resolve(Motion::UpdateNoticeRestartExit);
        let curve = Curve::Once(&exit);
        let times = curve.times(false);
        assert_eq!(times[0], 0.);
        assert_eq!(times[1], exit.delay_ms);
        assert_eq!(*times.last().unwrap(), exit.total_ms(false));
        assert_eq!(curve.pose(0., false).opacity, 1.);
        assert_eq!(curve.pose(exit.total_ms(false), false).opacity, 0.);
        // Reduced motion: one step at the delay.
        assert_eq!(curve.times(true), vec![0., exit.delay_ms]);
    }

    #[test]
    fn a_lifecycle_fades_in_holds_and_fades_out_by_its_close() {
        let life = resolve(Motion::RecordingControlsHiddenLifecycle);
        let curve = Curve::Lifecycle(&life, 6000.);
        assert_eq!(curve.pose(0., false).opacity, 0.);
        assert_eq!(curve.pose(3000., false), Pose::REST);
        assert!(curve.pose(5990., false).opacity < 0.1);
        // Reduced motion keeps it still and visible until it closes.
        assert_eq!(curve.times(true), vec![0.]);
        assert_eq!(curve.pose(0., true), Pose::REST);
    }
}
