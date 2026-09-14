use super::WindowPlacement;

/// Rectángulo visible de un monitor para validar la geometría de la ventana.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl DisplayBounds {
    #[must_use]
    const fn right(self) -> f32 {
        self.x + self.width
    }

    #[must_use]
    const fn bottom(self) -> f32 {
        self.y + self.height
    }

    /// Indica si el rectángulo comparte área con la ventana propuesta.
    #[must_use]
    fn overlaps(self, placement: WindowPlacement) -> bool {
        placement.x < self.right()
            && placement.x + placement.width > self.x
            && placement.y < self.bottom()
            && placement.y + placement.height > self.y
    }
}

const DEFAULT_WIDTH: f32 = 960.0;
const DEFAULT_HEIGHT: f32 = 640.0;
const MIN_WIDTH: f32 = 480.0;
const MIN_HEIGHT: f32 = 360.0;

/// Ajusta posición y tamaño para que la ventana quepa en algún monitor visible.
#[must_use]
pub fn validate_window_placement(
    placement: WindowPlacement,
    displays: &[DisplayBounds],
) -> WindowPlacement {
    let mut width = placement.width.clamp(MIN_WIDTH, DEFAULT_WIDTH * 2.0);
    let mut height = placement.height.clamp(MIN_HEIGHT, DEFAULT_HEIGHT * 2.0);

    if displays.is_empty() {
        return WindowPlacement {
            x: placement.x.max(0.0),
            y: placement.y.max(0.0),
            width,
            height,
        };
    }

    let target_display = displays
        .iter()
        .copied()
        .find(|display| display.overlaps(placement))
        .or_else(|| {
            let center_x = placement.x + placement.width * 0.5;
            let center_y = placement.y + placement.height * 0.5;
            displays.iter().copied().find(|display| {
                center_x >= display.x
                    && center_x <= display.right()
                    && center_y >= display.y
                    && center_y <= display.bottom()
            })
        })
        .unwrap_or(displays[0]);

    width = width.min(target_display.width);
    height = height.min(target_display.height);

    let mut x = placement.x;
    let mut y = placement.y;

    if x + width > target_display.right() {
        x = target_display.right() - width;
    }
    if y + height > target_display.bottom() {
        y = target_display.bottom() - height;
    }
    if x < target_display.x {
        x = target_display.x;
    }
    if y < target_display.y {
        y = target_display.y;
    }

    WindowPlacement {
        x,
        y,
        width,
        height,
    }
}

/// Geometría por defecto centrada en el monitor primario disponible.
#[must_use]
pub fn default_window_placement(displays: &[DisplayBounds]) -> WindowPlacement {
    let display = displays.first().copied().unwrap_or(DisplayBounds {
        x: 0.0,
        y: 0.0,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
    });
    WindowPlacement {
        x: display.x + (display.width - DEFAULT_WIDTH).max(0.0) * 0.5,
        y: display.y + (display.height - DEFAULT_HEIGHT).max(0.0) * 0.5,
        width: DEFAULT_WIDTH,
        height: DEFAULT_HEIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        DisplayBounds, WindowPlacement, default_window_placement, validate_window_placement,
    };

    const PRIMARY: DisplayBounds = DisplayBounds {
        x: 0.0,
        y: 0.0,
        width: 1920.0,
        height: 1080.0,
    };

    #[test]
    fn keeps_valid_placement_inside_display() {
        let placement = WindowPlacement {
            x: 100.0,
            y: 120.0,
            width: 960.0,
            height: 640.0,
        };

        let validated = validate_window_placement(placement, &[PRIMARY]);

        assert_eq!(validated, placement);
    }

    #[test]
    fn clamps_window_that_would_render_off_screen() {
        let placement = WindowPlacement {
            x: 1800.0,
            y: 900.0,
            width: 960.0,
            height: 640.0,
        };

        let validated = validate_window_placement(placement, &[PRIMARY]);

        assert!(validated.x + validated.width <= PRIMARY.right());
        assert!(validated.y + validated.height <= PRIMARY.bottom());
        assert!(validated.x >= PRIMARY.x);
        assert!(validated.y >= PRIMARY.y);
    }

    #[test]
    fn uses_primary_display_when_saved_monitor_is_gone() {
        let old_monitor = DisplayBounds {
            x: 4000.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let placement = WindowPlacement {
            x: 4100.0,
            y: 80.0,
            width: 960.0,
            height: 640.0,
        };

        let validated = validate_window_placement(placement, &[PRIMARY]);

        assert!(validated.x < PRIMARY.right());
        assert!((validated.x - placement.x).abs() > 1.0);
        let _ = old_monitor;
    }

    #[test]
    fn default_placement_fits_primary_display() {
        let placement = default_window_placement(&[PRIMARY]);

        assert!(placement.width <= PRIMARY.width);
        assert!(placement.height <= PRIMARY.height);
        assert!(placement.x >= PRIMARY.x);
        assert!(placement.y >= PRIMARY.y);
    }
}
