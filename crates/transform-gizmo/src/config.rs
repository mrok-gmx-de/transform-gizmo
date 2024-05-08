use std::ops::{Deref, DerefMut};

pub use ecolor::Color32;

use emath::Rect;
use enumset::{enum_set, EnumSet, EnumSetType};

use crate::math::{
    screen_to_world, world_to_screen, DMat4, DQuat, DVec3, DVec4, Transform, Vec4Swizzles,
};

/// The default snapping distance for rotation in radians
pub const DEFAULT_SNAP_ANGLE: f32 = std::f32::consts::PI / 32.0;
/// The default snapping distance for translation
pub const DEFAULT_SNAP_DISTANCE: f32 = 0.1;
/// The default snapping distance for scale
pub const DEFAULT_SNAP_SCALE: f32 = 0.1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AxisConfig {
    pub x: bool,
    pub y: bool,
    pub z: bool,
    pub view: bool,
}

impl Default for AxisConfig {
    fn default() -> AxisConfig {
        AxisConfig {
            x: true,
            y: true,
            z: true,
            view: true,
        }
    }
}

impl AxisConfig {
    pub fn is_active(&self, direction: GizmoDirection) -> bool {
        match direction {
            GizmoDirection::X => self.x,
            GizmoDirection::Y => self.y,
            GizmoDirection::Z => self.z,
            GizmoDirection::View => self.view,
        }
    }
}

/// Configuration of a gizmo.
///
/// Defines how the gizmo is drawn to the screen and
/// how it can be interacted with.
#[derive(Debug, Copy, Clone)]
pub struct GizmoConfig {
    /// View matrix for the gizmo, aligning it with the camera's viewpoint.
    pub view_matrix: mint::RowMatrix4<f64>,
    /// Projection matrix for the gizmo, determining how it is projected onto the screen.
    pub projection_matrix: mint::RowMatrix4<f64>,
    /// Screen area where the gizmo is displayed.
    pub viewport: Rect,
    /// The gizmo's operation modes.
    pub modes: EnumSet<GizmoMode>,
    /// Determines the gizmo's orientation relative to global or local axes.
    pub orientation: GizmoOrientation,
    /// Pivot point for transformations
    pub pivot_point: TransformPivotPoint,
    /// Toggles snapping to predefined increments during transformations for precision.
    pub snapping: bool,
    /// Angle increment for snapping rotations, in radians.
    pub snap_angle: f32,
    /// Distance increment for snapping translations.
    pub snap_distance: f32,
    /// Scale increment for snapping scalings.
    pub snap_scale: f32,
    /// Visual settings for the gizmo, affecting appearance and visibility.
    pub visuals: GizmoVisuals,
    /// Visibility of sub gizmo primitives
    pub gizmo_visibility: GizmoVisibility,
    /// Ratio of window's physical size to logical size.
    pub pixels_per_point: f32,
}

impl Default for GizmoConfig {
    fn default() -> Self {
        Self {
            view_matrix: DMat4::IDENTITY.into(),
            projection_matrix: DMat4::IDENTITY.into(),
            viewport: Rect::NOTHING,
            modes: enum_set!(GizmoMode::Rotate),
            orientation: GizmoOrientation::default(),
            pivot_point: TransformPivotPoint::default(),
            snapping: false,
            snap_angle: DEFAULT_SNAP_ANGLE,
            snap_distance: DEFAULT_SNAP_DISTANCE,
            snap_scale: DEFAULT_SNAP_SCALE,
            visuals: GizmoVisuals::default(),
            gizmo_visibility: GizmoVisibility::default(),
            pixels_per_point: 1.0,
        }
    }
}

impl GizmoConfig {
    /// Forward vector of the view camera
    pub(crate) fn view_forward(&self) -> DVec3 {
        DVec4::from(self.view_matrix.z).xyz()
    }

    /// Up vector of the view camera
    pub(crate) fn view_up(&self) -> DVec3 {
        DVec4::from(self.view_matrix.y).xyz()
    }

    /// Right vector of the view camera
    pub(crate) fn view_right(&self) -> DVec3 {
        DVec4::from(self.view_matrix.x).xyz()
    }

    /// Whether local orientation is used
    pub(crate) fn local_space(&self) -> bool {
        self.orientation() == GizmoOrientation::Local
    }

    /// Transform orientation of the gizmo
    pub(crate) fn orientation(&self) -> GizmoOrientation {
        if self.modes.contains(GizmoMode::Scale) {
            // Scaling currently only works in local orientation,
            // so the configured orientation is ignored.
            GizmoOrientation::Local
        } else {
            self.orientation
        }
    }
}

#[derive(Debug, Copy, Clone, Default)]
pub(crate) struct PreparedGizmoConfig {
    config: GizmoConfig,
    /// Rotation of the gizmo
    pub(crate) rotation: DQuat,
    /// Translation of the gizmo
    pub(crate) translation: DVec3,
    /// Scale of the gizmo
    pub(crate) scale: DVec3,
    /// Combined view-projection matrix
    pub(crate) view_projection: DMat4,
    /// Model matrix from targets
    pub(crate) model_matrix: DMat4,
    /// Combined model-view-projection matrix
    pub(crate) mvp: DMat4,
    /// Scale factor for the gizmo rendering
    pub(crate) scale_factor: f32,
    /// How close the mouse pointer needs to be to a subgizmo before it is focused
    pub(crate) focus_distance: f32,
    /// Whether left-handed projection is used
    pub(crate) left_handed: bool,
    /// Direction from the camera to the gizmo in world space
    pub(crate) eye_to_model_dir: DVec3,
}

impl Deref for PreparedGizmoConfig {
    type Target = GizmoConfig;

    fn deref(&self) -> &Self::Target {
        &self.config
    }
}

impl DerefMut for PreparedGizmoConfig {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.config
    }
}

impl PreparedGizmoConfig {
    pub(crate) fn update_for_config(&mut self, config: GizmoConfig) {
        let projection_matrix = DMat4::from(config.projection_matrix);
        let view_matrix = DMat4::from(config.view_matrix);

        let view_projection = projection_matrix * view_matrix;

        let left_handed = if projection_matrix.z_axis.w == 0.0 {
            projection_matrix.z_axis.z > 0.0
        } else {
            projection_matrix.z_axis.w > 0.0
        };

        self.config = config;
        self.view_projection = view_projection;
        self.left_handed = left_handed;

        self.update_transform(Transform {
            scale: self.scale.into(),
            rotation: self.rotation.into(),
            translation: self.translation.into(),
        });
    }

    pub(crate) fn update_for_targets(&mut self, targets: &[Transform]) {
        let mut scale = DVec3::ZERO;
        let mut translation = DVec3::ZERO;
        let mut rotation = DQuat::IDENTITY;

        let mut target_count = 0;
        for target in targets {
            scale += DVec3::from(target.scale);
            translation += DVec3::from(target.translation);
            rotation = DQuat::from(target.rotation);

            target_count += 1;
        }

        if target_count == 0 {
            scale = DVec3::ONE;
        } else {
            translation /= target_count as f64;
            scale /= target_count as f64;
        }

        self.update_transform(Transform {
            scale: scale.into(),
            rotation: rotation.into(),
            translation: translation.into(),
        });
    }

    pub(crate) fn update_transform(&mut self, transform: Transform) {
        self.translation = transform.translation.into();
        self.rotation = transform.rotation.into();
        self.scale = transform.scale.into();
        self.model_matrix =
            DMat4::from_scale_rotation_translation(self.scale, self.rotation, self.translation);
        self.mvp = self.view_projection * self.model_matrix;

        self.scale_factor = self.mvp.as_ref()[15] as f32
            / self.projection_matrix.x.x as f32
            / self.config.viewport.width()
            * 2.0;

        let gizmo_screen_pos =
            world_to_screen(self.config.viewport, self.mvp, self.translation).unwrap_or_default();

        let gizmo_view_near = screen_to_world(
            self.config.viewport,
            self.view_projection.inverse(),
            gizmo_screen_pos,
            -1.0,
        );

        self.focus_distance = self.scale_factor * (self.config.visuals.stroke_width / 2.0 + 5.0);

        self.eye_to_model_dir = (gizmo_view_near - self.translation).normalize_or_zero();
    }

    pub(crate) fn as_transform(&self) -> Transform {
        Transform {
            scale: self.scale.into(),
            rotation: self.rotation.into(),
            translation: self.translation.into(),
        }
    }
}

/// Operation mode of a gizmo.
#[derive(Debug, EnumSetType)]
pub enum GizmoMode {
    Rotate,
    Translate,
    Scale,
}

/// The point in space around which all rotations are centered.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum TransformPivotPoint {
    /// Pivot around the median point of targets
    #[default]
    MedianPoint,
    /// Pivot around each target's own origin
    IndividualOrigins,
}

/// Orientation of a gizmo.
#[derive(Debug, Copy, Clone, Eq, PartialEq, Default)]
pub enum GizmoOrientation {
    /// Transformation axes are aligned to world space.
    #[default]
    Global,
    /// Transformation axes are aligned to the last target's orientation.
    Local,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Hash)]
pub enum GizmoDirection {
    /// Gizmo points in the X-direction
    X,
    /// Gizmo points in the Y-direction
    Y,
    /// Gizmo points in the Z-direction
    Z,
    /// Gizmo points in the view direction
    View,
}

/// Controls the visual style of the gizmo
#[derive(Debug, Copy, Clone)]
pub struct GizmoVisuals {
    /// Color of the x axis
    pub x_color: Color32,
    /// Color of the y axis
    pub y_color: Color32,
    /// Color of the z axis
    pub z_color: Color32,
    /// Color of the forward axis
    pub s_color: Color32,
    /// Alpha of the gizmo color when inactive
    pub inactive_alpha: f32,
    /// Alpha of the gizmo color when highlighted/active
    pub highlight_alpha: f32,
    /// Color to use for highlighted and active axes. By default, the axis color is used with `highlight_alpha`
    pub highlight_color: Option<Color32>,
    /// Width (thickness) of the gizmo strokes
    pub stroke_width: f32,
    /// Gizmo size in pixels
    pub gizmo_size: f32,
}

impl Default for GizmoVisuals {
    fn default() -> Self {
        Self {
            x_color: Color32::from_rgb(255, 0, 125),
            y_color: Color32::from_rgb(0, 255, 125),
            z_color: Color32::from_rgb(0, 125, 255),
            s_color: Color32::from_rgb(255, 255, 255),
            inactive_alpha: 0.7,
            highlight_alpha: 1.0,
            highlight_color: None,
            stroke_width: 4.0,
            gizmo_size: 75.0,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct GizmoVisibility {
    // translation helper
    pub translation_arrow: AxisConfig,
    pub translation_plane: AxisConfig,
    // scaling helper
    pub scaling_arrow: AxisConfig,
    pub scaling_plane: AxisConfig,
    // Rotation helper
    pub rotation_arc: AxisConfig,
    pub rotation_arc_ball: bool,
}

impl Default for GizmoVisibility {
    fn default() -> Self {
        Self {
            translation_arrow: AxisConfig::default(),
            translation_plane: AxisConfig::default(),
            scaling_arrow: AxisConfig::default(),
            scaling_plane: AxisConfig::default(),
            rotation_arc: AxisConfig::default(),
            rotation_arc_ball: true,
        }
    }
}
