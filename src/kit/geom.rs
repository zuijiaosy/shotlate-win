//! Points and rects in a top-left-origin space. Capture views work in logical points
//! (physical pixels divided by the monitor's scale), like the macOS version.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const ZERO: Point = Point { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    pub fn offset(self, dx: f32, dy: f32) -> Point {
        Point::new(self.x + dx, self.y + dy)
    }

    pub fn distance(self, other: Point) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

impl std::ops::Add for Point {
    type Output = Point;
    fn add(self, o: Point) -> Point {
        Point::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Point {
    type Output = Point;
    fn sub(self, o: Point) -> Point {
        Point::new(self.x - o.x, self.y - o.y)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Size {
    pub width: f32,
    pub height: f32,
}

impl Size {
    pub const fn new(width: f32, height: f32) -> Size {
        Size { width, height }
    }
}

/// Always normalized: `width` and `height` are never negative. `Rect::NULL` is the empty union identity.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0.0, y: 0.0, width: 0.0, height: 0.0 };
    /// Nothing; unions ignore it and it contains nothing.
    pub const NULL: Rect = Rect { x: f32::INFINITY, y: f32::INFINITY, width: 0.0, height: 0.0 };

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect { x, y, width, height }
    }

    pub fn from_origin_size(origin: Point, size: Size) -> Rect {
        Rect::new(origin.x, origin.y, size.width, size.height)
    }

    /// The rect spanned by two opposite corners, in either order.
    pub fn from_corners(a: Point, b: Point) -> Rect {
        Rect::new(a.x.min(b.x), a.y.min(b.y), (a.x - b.x).abs(), (a.y - b.y).abs())
    }

    pub fn is_null(&self) -> bool {
        self.x.is_infinite() || self.y.is_infinite()
    }

    pub fn is_empty(&self) -> bool {
        self.is_null() || self.width <= 0.0 || self.height <= 0.0
    }

    pub fn min_x(&self) -> f32 {
        self.x
    }
    pub fn min_y(&self) -> f32 {
        self.y
    }
    pub fn max_x(&self) -> f32 {
        self.x + self.width
    }
    pub fn max_y(&self) -> f32 {
        self.y + self.height
    }
    pub fn mid_x(&self) -> f32 {
        self.x + self.width / 2.0
    }
    pub fn mid_y(&self) -> f32 {
        self.y + self.height / 2.0
    }
    pub fn origin(&self) -> Point {
        Point::new(self.x, self.y)
    }
    pub fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
    pub fn center(&self) -> Point {
        Point::new(self.mid_x(), self.mid_y())
    }

    /// Half-open on the far edges, like CoreGraphics.
    pub fn contains(&self, p: Point) -> bool {
        !self.is_null() && p.x >= self.x && p.x < self.max_x() && p.y >= self.y && p.y < self.max_y()
    }

    /// Positive `dx`/`dy` shrink, negative grow. Shrinking past zero gives NULL.
    pub fn inset(&self, dx: f32, dy: f32) -> Rect {
        if self.is_null() {
            return *self;
        }
        let r = Rect::new(self.x + dx, self.y + dy, self.width - 2.0 * dx, self.height - 2.0 * dy);
        if r.width < 0.0 || r.height < 0.0 { Rect::NULL } else { r }
    }

    pub fn offset(&self, dx: f32, dy: f32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.width, self.height)
    }

    pub fn union(&self, other: &Rect) -> Rect {
        if self.is_null() {
            return *other;
        }
        if other.is_null() {
            return *self;
        }
        let x = self.x.min(other.x);
        let y = self.y.min(other.y);
        Rect::new(x, y, self.max_x().max(other.max_x()) - x, self.max_y().max(other.max_y()) - y)
    }

    pub fn union_point(&self, p: Point) -> Rect {
        self.union(&Rect::new(p.x, p.y, 0.0, 0.0))
    }

    pub fn intersection(&self, other: &Rect) -> Rect {
        if self.is_null() || other.is_null() {
            return Rect::NULL;
        }
        let x = self.x.max(other.x);
        let y = self.y.max(other.y);
        let max_x = self.max_x().min(other.max_x());
        let max_y = self.max_y().min(other.max_y());
        if max_x < x || max_y < y { Rect::NULL } else { Rect::new(x, y, max_x - x, max_y - y) }
    }

    /// Smallest rect with whole-number edges that contains this one.
    pub fn integral(&self) -> Rect {
        if self.is_null() {
            return *self;
        }
        let x = self.x.floor();
        let y = self.y.floor();
        Rect::new(x, y, self.max_x().ceil() - x, self.max_y().ceil() - y)
    }

    pub fn scaled(&self, s: f32) -> Rect {
        Rect::new(self.x * s, self.y * s, self.width * s, self.height * s)
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn distance_to_segment(p: Point, a: Point, b: Point) -> f32 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let length_squared = dx * dx + dy * dy;
    if length_squared <= 0.0 {
        return p.distance(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / length_squared).clamp(0.0, 1.0);
    p.distance(Point::new(a.x + t * dx, a.y + t * dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn union_ignores_null() {
        let r = Rect::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(Rect::NULL.union(&r), r);
        assert_eq!(r.union(&Rect::NULL), r);
        assert!(Rect::NULL.is_null());
    }

    #[test]
    fn corners_normalize() {
        let r = Rect::from_corners(Point::new(10.0, 5.0), Point::new(2.0, 8.0));
        assert_eq!(r, Rect::new(2.0, 5.0, 8.0, 3.0));
    }

    #[test]
    fn intersection_of_disjoint_is_null() {
        let a = Rect::new(0.0, 0.0, 10.0, 10.0);
        let b = Rect::new(20.0, 20.0, 5.0, 5.0);
        assert!(a.intersection(&b).is_null());
        assert_eq!(a.intersection(&Rect::new(5.0, 5.0, 10.0, 10.0)), Rect::new(5.0, 5.0, 5.0, 5.0));
    }

    #[test]
    fn inset_past_zero_is_null() {
        assert!(Rect::new(0.0, 0.0, 4.0, 4.0).inset(3.0, 3.0).is_null());
        assert_eq!(Rect::new(0.0, 0.0, 4.0, 4.0).inset(-1.0, -2.0), Rect::new(-1.0, -2.0, 6.0, 8.0));
    }

    #[test]
    fn segment_distance() {
        let d = distance_to_segment(Point::new(5.0, 3.0), Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert!((d - 3.0).abs() < 1e-5);
        let end = distance_to_segment(Point::new(13.0, 4.0), Point::new(0.0, 0.0), Point::new(10.0, 0.0));
        assert!((end - 5.0).abs() < 1e-5);
    }
}
