//! The selection wheel (`COASKeyMappings`, `COASWheelScreen`, `SBOpen`): a key opens the station
//! of a stick the player carries. Tapped, or with one stick, it opens the first; held, it shows
//! every stick on a wheel, and letting go of the key or clicking opens the one under the
//! pointer. A right click switches between the round layout and the list.
use super::{ANVILS, ID, State, inventory_slots, transmute};
use mod_api::{
    ClientHost, Flow, Hand, InputKeyEvent, ItemResource, ItemStack, KeyAction, KeyRegistration,
    PayloadCodec, PayloadId, Registrar, ScreenMouseButtonPressedPreEvent, ScreenRenderPostEvent,
    ServerHost, Text, ToServer, TooltipElement,
};
use std::sync::{Arc, Mutex, OnceLock};

/// The key's id within the mod.
const KEY: &str = "open_curios";

/// `SBOpen`: the stick the player picked, by its wheel item's id (every anvil is the anvil).
#[derive(PayloadCodec)]
pub(crate) struct Pick {
    item: String,
}

/// The wheel's layouts (`WheelLayoutSupplier`); the polygonal one is unused in the original.
#[derive(Clone, Copy, Default, PartialEq)]
enum Layout {
    #[default]
    Round,
    List,
}

/// One option's area on the wheel, around its centre (`WheelPolygon`): its outline, which a
/// strip of quads fills from both ends, and where its item sits.
struct Area {
    points: Vec<[f32; 2]>,
    center: [f32; 2],
}

/// The wheel while it is up.
struct Up {
    /// Client ticks since it opened, for the fade (`openTick`).
    ticks: u32,
    /// The option under the pointer, by index and wheel item id.
    selected: usize,
    picked: Option<String>,
}

/// The client half: the key, the wheel and the last pick.
#[derive(Default)]
pub(crate) struct Wheel {
    send: OnceLock<ToServer<Pick>>,
    up: Mutex<Option<Up>>,
    layout: Mutex<Layout>,
    /// The last stick picked, first among the options next time (`lastSelection`).
    last: Mutex<Option<String>>,
    /// A click on the wheel, acted on at the next client tick (screen input hooks only read).
    clicked: Mutex<bool>,
}

impl Wheel {
    /// The key and both ends of the pick: the server opens the picked stick's station.
    pub(crate) fn register(&self, registrar: &mut dyn Registrar, state: Arc<State>) {
        registrar.register_key_mapping(
            KeyRegistration::new(KEY, "key.keyboard.v")
                .label("crafting_on_a_stick.key.open_curios")
                .category("key.categories.inventory"),
        );
        let mut payloads = registrar.payload_handlers();
        let mut view = payloads.registrar("1");
        let send = view.play_to_server_typed::<Pick>(
            PayloadId(format!("{ID}:sb_open")),
            move |pick, cx| {
                let player = cx.player().map_err(|e| format!("{e:?}"))?.uuid();
                let host = cx.server_host().map_err(|e| format!("{e:?}"))?;
                open_picked(&state, host, player, &pick.item);
                Ok(())
            },
        );
        let _ = self.send.set(send);
    }

    /// `ClientTickEvent.Pre`: the key opens the first stick or the wheel; a click on the wheel
    /// picks.
    pub(crate) fn tick(&self, state: &State, host: &mut dyn ClientHost) {
        if std::mem::take(&mut *self.clicked.lock().unwrap()) {
            self.pick(host);
        }
        if host.mod_screen() == Some(ID)
            && let Some(up) = self.up.lock().unwrap().as_mut()
        {
            up.ticks += 1;
        }
        if !host.consume_click(ID, KEY) || !host.playing() {
            return;
        }
        let options = self.options(state, host);
        let Some((first, _)) = options.first() else {
            return;
        };
        if options.len() == 1 || !host.is_down(ID, KEY) {
            *self.last.lock().unwrap() = Some(first.clone());
            self.send(host, first);
            return;
        }
        *self.up.lock().unwrap() = Some(Up {
            ticks: 0,
            selected: 0,
            picked: Some(first.clone()),
        });
        host.mod_screen_open(ID);
    }

    /// The wheel's `keyReleased`: letting go of the key picks.
    pub(crate) fn key(&self, host: &mut dyn ClientHost, event: &InputKeyEvent<'_>) {
        if event.action != KeyAction::Release || host.mod_screen() != Some(ID) {
            return;
        }
        if host
            .mod_key(ID, KEY)
            .is_some_and(|(key, _)| key == event.key)
        {
            self.pick(host);
        }
    }

    /// The wheel's `mouseClicked`: left picks, right switches the layout.
    pub(crate) fn click(
        &self,
        host: &dyn ClientHost,
        event: &ScreenMouseButtonPressedPreEvent<'_>,
    ) -> Flow {
        if host.mod_screen() != Some(ID) {
            return Flow::Continue;
        }
        match event.button {
            0 => *self.clicked.lock().unwrap() = true,
            1 => {
                let mut layout = self.layout.lock().unwrap();
                *layout = match *layout {
                    Layout::Round => Layout::List,
                    Layout::List => Layout::Round,
                };
            }
            _ => return Flow::Continue,
        }
        Flow::Cancel
    }

    /// `select`: the wheel closes and the stick under the pointer opens.
    fn pick(&self, host: &mut dyn ClientHost) {
        let up = self.up.lock().unwrap().take();
        host.mod_screen_close(ID);
        if let Some(item) = up.and_then(|up| up.picked) {
            *self.last.lock().unwrap() = Some(item.clone());
            self.send(host, &item);
        }
    }

    fn send(&self, host: &mut dyn ClientHost, item: &str) {
        if let Some(send) = self.send.get() {
            let _ = send.send(
                host,
                &Pick {
                    item: item.to_owned(),
                },
            );
        }
    }

    /// `getOptions`: one per wheel item the player carries (main inventory, then off hand), each
    /// with the stack it shows, the last pick first.
    fn options(&self, state: &State, host: &dyn ClientHost) -> Vec<(String, ItemStack)> {
        let Some(player) = host.local_player() else {
            return Vec::new();
        };
        let mut stacks = player.inventory(host).unwrap_or_default();
        stacks.extend(
            host.living(player.entity())
                .and_then(|living| living.hand_item(host, Hand::Off)),
        );
        let mut options: Vec<(String, ItemStack)> = Vec::new();
        for stack in stacks {
            let Some(path) = state.path(stack.item()) else {
                continue;
            };
            // `getWheelRepresentative`: every anvil shows as the anvil.
            let (path, shown) = if ANVILS.contains(&path) {
                let anvil = state.key("anvil");
                let shown = anvil.and_then(|anvil| transmute(host, &stack, anvil));
                ("anvil", shown.unwrap_or(stack))
            } else {
                (path, stack)
            };
            let id = format!("{ID}:{path}");
            if options.iter().all(|(o, _)| *o != id) {
                options.push((id, shown));
            }
        }
        if let Some(last) = self.last.lock().unwrap().as_ref()
            && let Some(at) = options.iter().position(|(o, _)| o == last)
        {
            let option = options.remove(at);
            options.insert(0, option);
        }
        options
    }

    /// `render`: the dimmed screen, an area per option with its item, the one nearest the
    /// pointer lit, and its tooltip once the wheel has faded in.
    pub(crate) fn render(
        &self,
        state: &State,
        host: &dyn ClientHost,
        event: &mut ScreenRenderPostEvent<'_>,
    ) {
        if host.mod_screen() != Some(ID) {
            return;
        }
        let mut up = self.up.lock().unwrap();
        let Some(up) = up.as_mut() else {
            return;
        };
        let options = self.options(state, host);
        // `getAnimationT`: hidden for 4 ticks, then fading in over 6.
        let t = ((up.ticks as f32 - 4.0) / 6.0).clamp(0.0, 1.0);
        let [width, height] = event.draw.size();
        event
            .draw
            .fill([0.0, 0.0, width, height], with_alpha(0x4000_0000, t));
        let areas = layout(*self.layout.lock().unwrap(), options.len());
        // `mouseMoved`: the option with the point nearest the pointer.
        let center = [width * 0.5, height * 0.5];
        let [mx, my] = event.screen.mouse;
        let (dx, dy) = (mx - center[0], my - center[1]);
        let mut nearest = f32::INFINITY;
        for (i, area) in areas.iter().enumerate() {
            for [x, y] in &area.points {
                let d = (x - dx) * (x - dx) + (y - dy) * (y - dy);
                if d < nearest {
                    nearest = d;
                    up.selected = i;
                }
            }
        }
        up.picked = options.get(up.selected).map(|(id, _)| id.clone());
        let scale = ease(t);
        let at = |[x, y]: [f32; 2]| [center[0] + x * scale, center[1] + y * scale];
        for (i, (area, (_, stack))) in areas.iter().zip(&options).enumerate() {
            let lit = up.selected == i;
            let base = if lit { 0xFFFF_FFFF } else { 0x80FF_FFFF };
            fill(&mut *event.draw, area, &at, with_alpha(base, t));
            let [x, y] = at(area.center);
            event
                .draw
                .item_stack(stack, [x - 8.0 * scale, y - 8.0 * scale], 16.0 * scale);
            if lit {
                fill(&mut *event.draw, area, &at, with_alpha(0x7FFF_FFFF, t));
            }
        }
        if t > 0.99
            && let Some((_, stack)) = options.get(up.selected)
        {
            let lines: Vec<Text> = ItemResource::of(stack)
                .tooltip(host, false, false)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|line| match line {
                    TooltipElement::Text(text, _) => Some(text),
                    _ => None,
                })
                .collect();
            let _ = event.draw.gui_tooltip(&lines, [mx, my]);
        }
    }
}

/// `SBOpen.handle`: the first stick in the player's inventory whose wheel item is `item` opens.
fn open_picked(state: &State, host: &mut dyn ServerHost, player: u128, item: &str) {
    let Some(path) = item.strip_prefix(&format!("{ID}:")) else {
        return;
    };
    let family: &[&str] = if path == "anvil" { &ANVILS } else { &[path] };
    for slot in inventory_slots() {
        let place = mod_api::ItemPlace::PlayerSlot { player, slot };
        let Some(stack) = place.stack(host) else {
            continue;
        };
        if state.path(stack.item()).is_some_and(|p| family.contains(&p)) {
            state.open(host, player, place, &stack);
            return;
        }
    }
}

/// `WheelPolygon.fill`: the outline as a strip of quads, each taking two points from either end.
fn fill(
    draw: &mut dyn mod_api::DrawContext,
    area: &Area,
    at: &dyn Fn([f32; 2]) -> [f32; 2],
    argb: u32,
) {
    let p = &area.points;
    let n = p.len();
    let mut i = 0;
    while 2 * i + 4 <= n {
        draw.quad([at(p[i]), at(p[i + 1]), at(p[n - i - 2]), at(p[n - i - 1])], argb);
        i += 1;
    }
}

/// `applyAlpha`: the colour's alpha scaled by `t`.
fn with_alpha(argb: u32, t: f32) -> u32 {
    let alpha = (((argb >> 24) as f32 * t) as u32) & 0xFF;
    (alpha << 24) | (argb & 0x00FF_FFFF)
}

/// `easeScale`: an ease-out that overshoots a little before it settles at 1.
fn ease(t: f32) -> f32 {
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

fn layout(layout: Layout, options: usize) -> Vec<Area> {
    match layout {
        Layout::Round => round(options),
        Layout::List => list(options),
    }
}

/// `RoundWheelLayout`: the first option in a disc at the centre, the others in a ring around
/// it, clockwise from the top, with a gap between them.
fn round(options: usize) -> Vec<Area> {
    const GAP: f32 = 2.0;
    const R1: f32 = 32.0;
    const R3: f32 = 64.0;
    const R0: f32 = R1 - 2.0 * GAP;
    const R2: f32 = (R1 + R3) / 2.0;
    // Fewer points than the original's 360: each quad is a draw call here.
    const CENTRAL: usize = 64;
    const HALF_AROUND: usize = 24;
    let point = |ratio: f32, distance: f32| {
        let rad = std::f32::consts::TAU * ratio;
        [distance * rad.sin(), -distance * rad.cos()]
    };
    if options == 0 {
        return Vec::new();
    }
    let around = options - 1;
    let mut areas = vec![Area {
        points: (0..CENTRAL)
            .map(|i| point(-(i as f32) / CENTRAL as f32, R0))
            .collect(),
        center: [0.0, 0.0],
    }];
    for index in 0..around {
        let gap = |r: f32| match around {
            1 => 0.0,
            _ => (GAP / r).asin() / std::f32::consts::TAU,
        };
        let (gap1, gap3) = (gap(R1), gap(R3));
        let n = around as f32;
        let (alpha1, beta1) = ((index as f32 - 0.5) / n + gap1, (index as f32 + 0.5) / n - gap1);
        let (alpha3, beta3) = ((index as f32 - 0.5) / n + gap3, (index as f32 + 0.5) / n - gap3);
        let mut points = vec![[0.0; 2]; HALF_AROUND * 2];
        for j in 0..HALF_AROUND {
            let t = j as f32 / (HALF_AROUND - 1) as f32;
            points[j] = point(alpha1 + (beta1 - alpha1) * t, R1);
            points[HALF_AROUND * 2 - j - 1] = point(alpha3 + (beta3 - alpha3) * t, R3);
        }
        areas.push(Area {
            points,
            center: point(index as f32 / n, R2),
        });
    }
    areas
}

/// `ListWheelLayout`: a row of squares, the first in the middle, the others to its left and
/// right.
fn list(options: usize) -> Vec<Area> {
    const RADIUS: f32 = 9.0;
    const SPACING: f32 = 4.0;
    if options == 0 {
        return Vec::new();
    }
    let around = options as i32 - 1;
    let left = around - around / 2;
    (0..options as i32)
        .map(|i| {
            let order = match i {
                0 => 0,
                i if i < left + 1 => i - left - 1,
                i => i - left,
            };
            let x = order as f32 * (SPACING + RADIUS * 2.0);
            Area {
                points: vec![
                    [x - RADIUS, -RADIUS],
                    [x - RADIUS, RADIUS],
                    [x + RADIUS, RADIUS],
                    [x + RADIUS, -RADIUS],
                ],
                center: [x, 0.0],
            }
        })
        .collect()
}
