use ecolor::Rgba;
use emath::Pos2;
use std::ops::{Add, AddAssign, Sub};

use crate::config::{
    GizmoConfig, GizmoDirection, GizmoMode, PreparedGizmoConfig, TransformPivotPoint,
};
use crate::math::{screen_to_world, Transform};
use crate::GizmoOrientation;
use epaint::Mesh;
use glam::{DQuat, DVec3};

use crate::subgizmo::rotation::RotationParams;
use crate::subgizmo::scale::ScaleParams;
use crate::subgizmo::translation::TranslationParams;
use crate::subgizmo::{
    common::TransformKind, ArcballSubGizmo, RotationSubGizmo, ScaleSubGizmo, SubGizmo,
    SubGizmoControl, TranslationSubGizmo,
};

/// A 3D transformation gizmo.
#[derive(Clone, Debug, Default)]
pub struct Gizmo {
    /// Prepared configuration of the gizmo.
    /// Includes the original [`GizmoConfig`] as well as
    /// various other values calculated from it, used for
    /// interaction and drawing the gizmo.
    config: PreparedGizmoConfig,
    /// Subgizmos used in the gizmo.
    subgizmos: Vec<SubGizmo>,
    active_subgizmo_id: Option<u64>,

    target_start_transforms: Vec<Transform>,

    gizmo_start_transform: Transform,
}

impl Gizmo {
    /// Creates a new gizmo from given configuration
    pub fn new(config: GizmoConfig) -> Self {
        let mut gizmo = Self::default();
        gizmo.update_config(config);
        gizmo
    }

    /// Current configuration used by the gizmo.
    pub fn config(&self) -> &GizmoConfig {
        &self.config
    }

    /// Updates the configuration used by the gizmo.
    pub fn update_config(&mut self, config: GizmoConfig) {
        if config.modes != self.config.modes
            || config.gizmo_visibility != self.config.gizmo_visibility
        {
            self.subgizmos.clear();
            self.active_subgizmo_id = None;
        }

        self.config.update_for_config(config);

        if self.subgizmos.is_empty() {
            for mode in self.config.modes {
                match mode {
                    GizmoMode::Rotate => {
                        self.add_rotation();
                    }
                    GizmoMode::Translate => {
                        self.add_translation();
                    }
                    GizmoMode::Scale => {
                        self.add_scale();
                    }
                };
            }
        }
    }

    /// Was this gizmo focused after the latest [`Gizmo::update`] call.
    pub fn is_focused(&self) -> bool {
        self.subgizmos.iter().any(|subgizmo| subgizmo.is_focused())
    }

    /// Updates the gizmo based on given interaction information.
    ///
    /// # Examples
    ///
    /// ```
    /// # // Dummy values
    /// # use transform_gizmo::GizmoInteraction;
    /// # let mut gizmo = transform_gizmo::Gizmo::default();
    /// # let cursor_pos = Default::default();
    /// # let drag_started = true;
    /// # let dragging = true;
    /// # let mut transforms = vec![];
    ///
    /// let interaction = GizmoInteraction {
    ///     cursor_pos,
    ///     drag_started,
    ///     dragging
    /// };
    ///
    /// if let Some((_result, new_transforms)) = gizmo.update(interaction, &transforms) {
    ///                 for (new_transform, transform) in
    ///     // Update transforms
    ///     new_transforms.iter().zip(&mut transforms)
    ///     {
    ///         *transform = *new_transform;
    ///     }
    /// }
    /// ```
    ///
    /// Returns the result of the interaction with the updated transformation.
    ///
    /// [`Some`] is returned when any of the subgizmos is being dragged, [`None`] otherwise.
    pub fn update(
        &mut self,
        interaction: GizmoInteraction,
        targets: &[Transform],
    ) -> Option<(GizmoResult, Vec<Transform>)> {
        if !self.config.viewport.is_finite() {
            return None;
        }

        // Update the gizmo based on the given target transforms,
        // unless the gizmo is currently being interacted with.
        if self.active_subgizmo_id.is_none() {
            self.config.update_for_targets(targets);
        }

        for subgizmo in &mut self.subgizmos {
            // Update current configuration to each subgizmo.
            subgizmo.update_config(self.config);
            // All subgizmos are initially considered unfocused.
            subgizmo.set_focused(false);
        }

        let pointer_ray = self.pointer_ray(Pos2::from(interaction.cursor_pos));

        // If there is no active subgizmo, find which one of them
        // is under the mouse pointer, if any.
        if self.active_subgizmo_id.is_none() {
            if let Some(subgizmo) = self.pick_subgizmo(pointer_ray) {
                subgizmo.set_focused(true);

                // If we started dragging from one of the subgizmos, mark it as active.
                if interaction.drag_started {
                    self.active_subgizmo_id = Some(subgizmo.id());
                    self.target_start_transforms = targets.to_vec();
                    self.gizmo_start_transform = self.config.as_transform();
                }
            }
        }

        let mut result = None;

        if let Some(subgizmo) = self.active_subgizmo_mut() {
            if interaction.dragging {
                subgizmo.set_active(true);
                subgizmo.set_focused(true);
                result = subgizmo.update(pointer_ray);
            } else {
                subgizmo.set_active(false);
                subgizmo.set_focused(false);
                self.active_subgizmo_id = None;
            }
        }

        let Some(result) = result else {
            // No interaction, no result.

            self.config.update_for_targets(targets);

            for subgizmo in &mut self.subgizmos {
                subgizmo.update_config(self.config);
            }

            return None;
        };

        self.update_config_with_result(result);

        let updated_targets =
            self.update_transforms_with_result(result, targets, &self.target_start_transforms);

        Some((result, updated_targets))
    }

    /// Return all the necessary data to draw the latest gizmo interaction.
    ///
    /// The gizmo draw data consists of vertices in viewport coordinates.
    pub fn draw(&self) -> GizmoDrawData {
        if !self.config.viewport.is_finite() {
            return GizmoDrawData::default();
        }

        let mut draw_data = GizmoDrawData::default();
        for subgizmo in &self.subgizmos {
            if self.active_subgizmo_id.is_none() || subgizmo.is_active() {
                draw_data += subgizmo.draw();
            }
        }

        draw_data
    }

    fn active_subgizmo_mut(&mut self) -> Option<&mut SubGizmo> {
        self.active_subgizmo_id.and_then(|id| {
            self.subgizmos
                .iter_mut()
                .find(|subgizmo| subgizmo.id() == id)
        })
    }

    fn update_transforms_with_result(
        &self,
        result: GizmoResult,
        transforms: &[Transform],
        start_transforms: &[Transform],
    ) -> Vec<Transform> {
        transforms
            .iter()
            .zip(start_transforms)
            .map(|(transform, start_transform)| match result {
                GizmoResult::Rotation {
                    axis,
                    delta,
                    total: _,
                    is_view_axis,
                } => self.update_rotation(transform, axis, delta, is_view_axis),
                GizmoResult::Translation { delta, total: _ } => {
                    self.update_translation(delta, transform, start_transform)
                }
                GizmoResult::Scale { total } => {
                    Self::update_scale(transform, start_transform, total)
                }
                GizmoResult::Arcball { delta, total: _ } => {
                    self.update_rotation_quat(transform, delta.into())
                }
            })
            .collect()
    }

    fn update_rotation(
        &self,
        transform: &Transform,
        axis: mint::Vector3<f64>,
        delta: f64,
        is_view_axis: bool,
    ) -> Transform {
        let axis = match self.config.orientation() {
            GizmoOrientation::Local if !is_view_axis => {
                DQuat::from(transform.rotation) * DVec3::from(axis)
            }
            _ => DVec3::from(axis),
        };

        let delta = DQuat::from_axis_angle(axis, delta);

        self.update_rotation_quat(transform, delta)
    }

    fn update_rotation_quat(&self, transform: &Transform, delta: DQuat) -> Transform {
        let translation = match self.config.pivot_point {
            TransformPivotPoint::MedianPoint => (self.config.translation
                + delta * (DVec3::from(transform.translation) - self.config.translation))
                .into(),
            TransformPivotPoint::IndividualOrigins => transform.translation,
        };

        Transform {
            scale: transform.scale,
            rotation: (delta * DQuat::from(transform.rotation)).into(),
            translation,
        }
    }

    fn update_translation(
        &self,
        delta: mint::Vector3<f64>,
        transform: &Transform,
        start_transform: &Transform,
    ) -> Transform {
        let delta = match self.config.orientation() {
            GizmoOrientation::Global => DVec3::from(delta),
            GizmoOrientation::Local => DQuat::from(start_transform.rotation) * DVec3::from(delta),
        };

        Transform {
            scale: start_transform.scale,
            rotation: start_transform.rotation,
            translation: (delta + DVec3::from(transform.translation)).into(),
        }
    }

    fn update_scale(
        transform: &Transform,
        start_transform: &Transform,
        scale: mint::Vector3<f64>,
    ) -> Transform {
        Transform {
            scale: (DVec3::from(start_transform.scale) * DVec3::from(scale)).into(),
            rotation: transform.rotation,
            translation: transform.translation,
        }
    }

    fn update_config_with_result(&mut self, result: GizmoResult) {
        let new_config_transform = self.update_transforms_with_result(
            result,
            &[self.config.as_transform()],
            &[self.gizmo_start_transform],
        )[0];

        self.config.update_transform(new_config_transform);
    }

    /// Picks the subgizmo that is closest to the given world space ray.
    fn pick_subgizmo(&mut self, ray: Ray) -> Option<&mut SubGizmo> {
        self.subgizmos
            .iter_mut()
            .filter_map(|subgizmo| subgizmo.pick(ray).map(|t| (t, subgizmo)))
            .min_by(|(first, _), (second, _)| {
                first
                    .partial_cmp(second)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(_, subgizmo)| subgizmo)
    }

    /// Adds rotation subgizmos
    fn add_rotation(&mut self) {
        self.subgizmos.extend(
            [
                (
                    GizmoDirection::X,
                    RotationParams {
                        direction: GizmoDirection::X,
                    },
                ),
                (
                    GizmoDirection::Y,
                    RotationParams {
                        direction: GizmoDirection::Y,
                    },
                ),
                (
                    GizmoDirection::Z,
                    RotationParams {
                        direction: GizmoDirection::Z,
                    },
                ),
                (
                    GizmoDirection::View,
                    RotationParams {
                        direction: GizmoDirection::View,
                    },
                ),
            ]
            .iter()
            .filter_map(|&(direction, params)| {
                if self
                    .config
                    .gizmo_visibility
                    .rotation_arc
                    .is_active(direction)
                {
                    Some(RotationSubGizmo::new(self.config, params).into())
                } else {
                    None
                }
            }),
        );
        if self.config.gizmo_visibility.rotation_arc_ball {
            self.subgizmos
                .push(ArcballSubGizmo::new(self.config, ()).into());
        }
    }

    /// Adds translation subgizmos
    fn add_translation(&mut self) {
        self.subgizmos.extend(
            [
                (
                    GizmoDirection::X,
                    TranslationParams {
                        direction: GizmoDirection::X,
                        transform_kind: TransformKind::Axis,
                    },
                ),
                (
                    GizmoDirection::Y,
                    TranslationParams {
                        direction: GizmoDirection::Y,
                        transform_kind: TransformKind::Axis,
                    },
                ),
                (
                    GizmoDirection::Z,
                    TranslationParams {
                        direction: GizmoDirection::Z,
                        transform_kind: TransformKind::Axis,
                    },
                ),
                (
                    GizmoDirection::View,
                    TranslationParams {
                        direction: GizmoDirection::View,
                        transform_kind: TransformKind::Plane,
                    },
                ),
            ]
            .iter()
            .filter_map(|&(direction, params)| {
                if self
                    .config
                    .gizmo_visibility
                    .translation_arrow
                    .is_active(direction)
                {
                    Some(TranslationSubGizmo::new(self.config, params).into())
                } else {
                    None
                }
            }),
        );

        // Plane subgizmos are not added when both translation and scaling are enabled.
        if !self.config.modes.contains(GizmoMode::Scale) {
            self.subgizmos.extend(
                [
                    (
                        GizmoDirection::X,
                        TranslationParams {
                            direction: GizmoDirection::X,
                            transform_kind: TransformKind::Plane,
                        },
                    ),
                    (
                        GizmoDirection::Y,
                        TranslationParams {
                            direction: GizmoDirection::Y,
                            transform_kind: TransformKind::Plane,
                        },
                    ),
                    (
                        GizmoDirection::Z,
                        TranslationParams {
                            direction: GizmoDirection::Z,
                            transform_kind: TransformKind::Plane,
                        },
                    ),
                ]
                .iter()
                .filter_map(|&(direction, params)| {
                    if self
                        .config
                        .gizmo_visibility
                        .translation_plane
                        .is_active(direction)
                    {
                        Some(TranslationSubGizmo::new(self.config, params).into())
                    } else {
                        None
                    }
                }),
            );
        }
    }

    /// Adds scale subgizmos
    fn add_scale(&mut self) {
        self.subgizmos.extend(
            [
                (
                    GizmoDirection::X,
                    ScaleParams {
                        direction: GizmoDirection::X,
                        transform_kind: TransformKind::Axis,
                    },
                ),
                (
                    GizmoDirection::Y,
                    ScaleParams {
                        direction: GizmoDirection::Y,
                        transform_kind: TransformKind::Axis,
                    },
                ),
                (
                    GizmoDirection::Z,
                    ScaleParams {
                        direction: GizmoDirection::Z,
                        transform_kind: TransformKind::Axis,
                    },
                ),
            ]
            .iter()
            .filter_map(|&(direction, params)| {
                if self
                    .config
                    .gizmo_visibility
                    .scaling_arrow
                    .is_active(direction)
                {
                    Some(ScaleSubGizmo::new(self.config, params).into())
                } else {
                    None
                }
            }),
        );

        // Uniform scaling subgizmo is added when only scaling is enabled.
        // Otherwise it would overlap with rotation or translation subgizmos.
        if self.config.modes.len() == 1 && self.config.gizmo_visibility.scaling_plane.view {
            self.subgizmos.push(
                ScaleSubGizmo::new(
                    self.config,
                    ScaleParams {
                        direction: GizmoDirection::View,
                        transform_kind: TransformKind::Plane,
                    },
                )
                .into(),
            );
        }

        // Plane subgizmos are not added when both translation and scaling are enabled.
        if !self.config.modes.contains(GizmoMode::Translate) {
            self.subgizmos.extend(
                [
                    (
                        GizmoDirection::X,
                        ScaleParams {
                            direction: GizmoDirection::X,
                            transform_kind: TransformKind::Plane,
                        },
                    ),
                    (
                        GizmoDirection::Y,
                        ScaleParams {
                            direction: GizmoDirection::Y,
                            transform_kind: TransformKind::Plane,
                        },
                    ),
                    (
                        GizmoDirection::Z,
                        ScaleParams {
                            direction: GizmoDirection::Z,
                            transform_kind: TransformKind::Plane,
                        },
                    ),
                ]
                .iter()
                .filter_map(|&(direction, params)| {
                    if self
                        .config
                        .gizmo_visibility
                        .translation_plane
                        .is_active(direction)
                    {
                        Some(ScaleSubGizmo::new(self.config, params).into())
                    } else {
                        None
                    }
                }),
            )
        }
    }

    /// Calculate a world space ray from given screen space position
    fn pointer_ray(&self, screen_pos: Pos2) -> Ray {
        let mat = self.config.view_projection.inverse();
        let origin = screen_to_world(self.config.viewport, mat, screen_pos, -1.0);
        let target = screen_to_world(self.config.viewport, mat, screen_pos, 1.0);

        let direction = target.sub(origin).normalize();

        Ray {
            screen_pos,
            origin,
            direction,
        }
    }
}

/// Information needed for interacting with the gizmo.
#[derive(Default, Clone, Copy, Debug)]
pub struct GizmoInteraction {
    /// Current cursor position in window coordinates.
    pub cursor_pos: (f32, f32),
    /// Whether dragging was started this frame.
    /// Usually this is set to true if the primary mouse
    /// button was just pressed.
    pub drag_started: bool,
    /// Whether the user is currently dragging.
    /// Usually this is set to true whenever the primary mouse
    /// button is being pressed.
    pub dragging: bool,
}

/// Result of a gizmo transformation
#[derive(Debug, Copy, Clone)]
pub enum GizmoResult {
    Rotation {
        /// The rotation axis,
        axis: mint::Vector3<f64>,
        /// The latest rotation angle delta
        delta: f64,
        /// Total rotation angle of the gizmo interaction
        total: f64,
        /// Whether we are rotating along the view axis
        is_view_axis: bool,
    },
    Translation {
        /// The latest translation delta
        delta: mint::Vector3<f64>,
        /// Total translation of the gizmo interaction
        total: mint::Vector3<f64>,
    },
    Scale {
        /// Total scale of the gizmo interaction
        total: mint::Vector3<f64>,
    },
    Arcball {
        /// The latest rotation delta
        delta: mint::Quaternion<f64>,
        /// Total rotation of the gizmo interaction
        total: mint::Quaternion<f64>,
    },
}

/// Data used to draw [`Gizmo`].
#[derive(Default, Clone, Debug)]
pub struct GizmoDrawData {
    /// Vertices in viewport space.
    pub vertices: Vec<[f32; 2]>,
    /// Linear RGBA colors.
    pub colors: Vec<[f32; 4]>,
    /// Indices to the vertex data.
    pub indices: Vec<u32>,
}

impl From<Mesh> for GizmoDrawData {
    fn from(mesh: Mesh) -> Self {
        let (vertices, colors): (Vec<_>, Vec<_>) = mesh
            .vertices
            .iter()
            .map(|vertex| {
                (
                    [vertex.pos.x, vertex.pos.y],
                    Rgba::from(vertex.color).to_array(),
                )
            })
            .unzip();

        Self {
            vertices,
            colors,
            indices: mesh.indices,
        }
    }
}

impl AddAssign for GizmoDrawData {
    fn add_assign(&mut self, rhs: Self) {
        let index_offset = self.vertices.len() as u32;
        self.vertices.extend(rhs.vertices);
        self.colors.extend(rhs.colors);
        self.indices
            .extend(rhs.indices.into_iter().map(|idx| index_offset + idx));
    }
}

impl Add for GizmoDrawData {
    type Output = Self;

    fn add(mut self, rhs: Self) -> Self::Output {
        self += rhs;
        self
    }
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Ray {
    pub(crate) screen_pos: Pos2,
    pub(crate) origin: DVec3,
    pub(crate) direction: DVec3,
}
