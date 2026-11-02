//! NULL-backend smoke test of the safe Raven wrapper.
//!
//! There is one `#[test]`, because Raven's UI thread is the thread that called `init`: every
//! check runs in order on this thread. Injected input goes through Raven's real event path
//! (the NULL platform routes it to the widgets, which Raven's own test suite relies on too).
//!
//! `native_error_box` is the one wrapper call left out: it opens a modal `MessageBoxW` that
//! waits for a person to answer it.

use savestates_ui_raven::{
    Backend, Event, MouseButton, Rgba, Widget, Window, bind_action, combo, custom, dialog,
    draw_frame, init, is_ui_thread, list, locale, mono_text_size, open_url, quit, shutdown, tabs,
    task, text_width, theme, tr, tr_fmt, tr_plural, tree, wait_frame, widget_from_xml,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

const MAIN: &str = r#"
<Window Title="smoke" Width="600" Height="500">
    <VBox Spacing="4">
        <Button Name="go" Text="Go" OnClicked="t.go"/>
        <Label Name="msg"/>
        <TextInput Name="input"/>
        <ListView Name="files" Height="90"/>
        <TreeView Name="folders" Height="70"/>
        <ComboBox Name="choice"/>
        <TabView Name="pages" Height="60"/>
        <Custom Name="canvas" Slot.Fill="1"/>
        <Overlay Name="host" Slot.Fill="1"/>
    </VBox>
</Window>"#;

const CSV: &str =
    "key,pt-BR,en\nhello,Olá,Hello\nitems.one,{n} item,{n} item\nitems.other,{n} itens,{n} items\n";

/// Runs frames until `done` holds, at most `max` of them. Returns whether it held.
fn frames_until(max: usize, done: impl Fn() -> bool) -> bool {
    for _ in 0..max {
        if done() {
            return true;
        }
        draw_frame();
    }
    done()
}

fn center(w: Widget) -> (f32, f32) {
    let (x, y, width, height) = w.rect();
    (x + width / 2.0, y + height / 2.0)
}

fn click(win: Window, x: f32, y: f32) {
    win.inject_mouse(x, y, Some(true));
    win.inject_mouse(x, y, Some(false));
}

#[test]
fn smoke() {
    // 1. Runtime: init on the NULL backend; a second init reports Raven's error text.
    init(Backend::Null).expect("init on the NULL backend");
    assert!(is_ui_thread());
    let again = init(Backend::Null).expect_err("a second init fails");
    assert!(again.contains("already initialized"), "error text: {again}");
    assert!(
        !open_url(""),
        "an empty address is refused without asking the OS"
    );

    // 2. Windows: XML loading, finding widgets, geometry; errors carry Raven's text.
    let win = Window::from_xml(MAIN).expect("main window");
    let go = win.find("go").expect("the go button");
    assert_eq!(win.find("nope"), None);
    let err = Window::from_xml("<Window><Unclosed></Window>").expect_err("broken XML");
    assert!(err.contains("R_ERROR_XML"), "error text: {err}");
    let err = widget_from_xml("<VBox>").expect_err("broken XML");
    assert!(err.contains("R_ERROR_XML"), "error text: {err}");
    assert!(draw_frame());
    assert_eq!(win.size(), (600, 500));
    win.set_size(640, 520);
    draw_frame();
    assert_eq!(win.size(), (640, 520));
    let _ = win.position();
    win.set_position(40, 30);
    win.show();
    draw_frame();
    assert!(!win.is_maximized());
    let closes = Rc::new(Cell::new(0));
    win.on_close({
        let closes = closes.clone();
        move || {
            closes.set(closes.get() + 1);
            true
        }
    });
    let focus_gained = Rc::new(Cell::new(0));
    win.on_focus_gained({
        let focus_gained = focus_gained.clone();
        move || focus_gained.set(focus_gained.get() + 1)
    });

    // 3. Actions: a click injected at the center of the button fires its OnClicked action.
    let clicks = Rc::new(Cell::new(0));
    bind_action("t.go", {
        let clicks = clicks.clone();
        move |sender| {
            assert_eq!(sender, go);
            clicks.set(clicks.get() + 1);
        }
    });
    let (gx, gy) = center(go);
    click(win, gx, gy);
    assert!(
        frames_until(5, || clicks.get() == 1),
        "clicks: {}",
        clicks.get()
    );
    // A disabled button ignores the click; enabled again, it works.
    go.set_enabled(false);
    click(win, gx, gy);
    draw_frame();
    assert_eq!(clicks.get(), 1);
    go.set_enabled(true);
    click(win, gx, gy);
    assert!(frames_until(5, || clicks.get() == 2));

    // Widget calls.
    let msg = win.find("msg").expect("msg label");
    msg.set_label_text("hello");
    msg.set_tooltip(Some("a tip"));
    msg.set_tooltip(None);
    msg.set_visible(false);
    msg.set_visible(true);
    msg.invalidate();
    go.set_button_text("Go!");
    let input = win.find("input").expect("text input");
    input.text_input_set("typed text");
    assert_eq!(input.text_input_get(), "typed text");
    // Edits the user makes are reported; text set from code is not.
    let edits: Rc<RefCell<Vec<String>>> = Rc::default();
    input.on_text_changed({
        let edits = edits.clone();
        move |text| edits.borrow_mut().push(text)
    });
    input.text_input_set("");
    let (ix, iy) = center(input);
    click(win, ix, iy);
    draw_frame();
    win.inject_text("ab");
    draw_frame();
    assert_eq!(input.text_input_get(), "ab");
    assert_eq!(edits.borrow().last().map(String::as_str), Some("ab"));
    let reported = edits.borrow().len();
    input.text_input_set("typed text");
    assert_eq!(edits.borrow().len(), reported);

    // 4. Translations.
    assert_eq!(locale::load_csv(CSV), Ok(3));
    // A CRLF checkout must not leave "\r" in the texts, quoted multi-line ones included.
    let crlf =
        "key,pt-BR,en\ngreet,Olá {0},Hello {0}\ncrlf,Sim,Yes\nmulti,\"um\ndois\",\"one\ntwo\"\n"
            .replace('\n', "\r\n");
    assert_eq!(locale::load_csv(&crlf), Ok(3));
    locale::set_fallback("pt-BR");
    let changes = Rc::new(Cell::new(0));
    locale::on_changed({
        let changes = changes.clone();
        move || changes.set(changes.get() + 1)
    });
    locale::set_language("en");
    assert_eq!(tr("hello"), "Hello");
    assert_eq!(tr("crlf"), "Yes");
    assert_eq!(tr("multi"), "one\ntwo");
    assert_eq!(tr_fmt("greet", &["Ana"]), "Hello Ana");
    assert_eq!(tr_plural("items", 1), "1 item");
    input.set_text_key("Text", "hello");
    assert_eq!(input.text_input_get(), "Hello");
    locale::set_language("pt-BR");
    assert_eq!(tr("hello"), "Olá");
    assert_eq!(tr_plural("items", 2), "2 itens");
    assert_eq!(tr_fmt("greet", &["Ana"]), "Olá Ana");
    assert_eq!(
        input.text_input_get(),
        "Olá",
        "the key follows the language"
    );
    assert!(changes.get() >= 2, "changes: {}", changes.get());
    assert_eq!(tr("missing.key"), "missing.key");

    // 5. Tasks: post from another thread runs on the UI thread during a frame.
    let posted: Arc<Mutex<Option<bool>>> = Arc::new(Mutex::new(None));
    {
        let posted = posted.clone();
        std::thread::spawn(move || {
            task::post(move || *posted.lock().unwrap() = Some(is_ui_thread()));
        })
        .join()
        .unwrap();
    }
    assert!(frames_until(5, || posted.lock().unwrap().is_some()));
    assert_eq!(*posted.lock().unwrap(), Some(true));
    // run_async runs on a worker, which posts its result back to the UI thread.
    let round_trip: Arc<Mutex<Option<(bool, bool)>>> = Arc::new(Mutex::new(None));
    {
        let round_trip = round_trip.clone();
        task::run_async(move || {
            let on_worker = is_ui_thread();
            task::post(move || *round_trip.lock().unwrap() = Some((on_worker, is_ui_thread())));
        });
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while round_trip.lock().unwrap().is_none() && std::time::Instant::now() < deadline {
        wait_frame(0.05);
    }
    assert_eq!(*round_trip.lock().unwrap(), Some((false, true)));
    // Timers: a one-shot fires once; a cancelled repeating one stops.
    let once = Rc::new(Cell::new(0));
    let _ = task::timer(0.0, false, {
        let once = once.clone();
        move || once.set(once.get() + 1)
    });
    let ticks = Rc::new(Cell::new(0));
    let ticker = task::timer(0.001, true, {
        let ticks = ticks.clone();
        move || ticks.set(ticks.get() + 1)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while (once.get() == 0 || ticks.get() < 2) && std::time::Instant::now() < deadline {
        wait_frame(0.01);
    }
    assert_eq!(once.get(), 1);
    assert!(ticks.get() >= 2, "ticks: {}", ticks.get());
    ticker.cancel();
    let after_cancel = ticks.get();
    for _ in 0..5 {
        wait_frame(0.005);
    }
    assert_eq!(ticks.get(), after_cancel);
    assert_eq!(once.get(), 1);

    // 6. Custom painting and events, with local coordinates.
    let canvas = win.find("canvas").expect("custom canvas");
    let paints: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
    custom::on_paint(canvas, {
        let paints = paints.clone();
        move |p, w, h| {
            let c = Rgba(0.2, 0.7, 0.9, 1.0);
            p.push_clip(0.0, 0.0, w, h);
            p.rect(0.0, 0.0, w, h, 4.0, Rgba(0.05, 0.07, 0.12, 1.0));
            p.rect_outline(2.0, 2.0, w - 4.0, h - 4.0, 4.0, 1.5, c);
            p.line(0.0, h / 2.0, w, h / 2.0, 3.0, c);
            p.circle(w / 2.0, h / 2.0, 7.0, c);
            p.curve(
                (0.0, 0.0),
                (w / 3.0, h),
                (w * 2.0 / 3.0, 0.0),
                (w, h),
                2.0,
                c,
            );
            p.text(13.0, 8.0, 8.0, "Save #1", c);
            p.mono_text(13.0, 8.0, 24.0, "gap: 12px;", c);
            p.pop_clip();
            paints.borrow_mut().push((w, h));
        }
    });
    let events: Rc<RefCell<Vec<Event>>> = Rc::default();
    custom::on_event(canvas, {
        let events = events.clone();
        move |e| {
            events.borrow_mut().push(e.clone());
            true
        }
    });
    canvas.invalidate();
    assert!(frames_until(5, || !paints.borrow().is_empty()));
    let (cx, cy, cw, ch) = canvas.rect();
    let (pw, ph) = paints.borrow()[0];
    assert!(pw > 0.0 && ph > 0.0 && (pw - cw).abs() < 1.0 && (ph - ch).abs() < 1.0);
    assert!(text_width(13.0, "Save #1") > text_width(13.0, "S"));
    // The fixed-width font gives every character the same width.
    let (narrow, line_h) = mono_text_size(13.0, "iiii");
    let (wide, _) = mono_text_size(13.0, "MMMM");
    assert!(narrow > 0.0 && line_h > 0.0);
    assert!((narrow - wide).abs() < 0.01, "{narrow} vs {wide}");
    assert!(text_width(13.0, "iiii") < text_width(13.0, "MMMM"));

    win.inject_wheel(cx + 30.0, cy + 20.0, 1.0, true);
    draw_frame();
    let wheel = events
        .borrow()
        .iter()
        .find(|e| matches!(e, Event::Wheel { .. }))
        .cloned();
    assert_eq!(
        wheel,
        Some(Event::Wheel {
            x: 30.0,
            y: 20.0,
            wx: 0.0,
            wy: 1.0,
            ctrl: true
        })
    );
    win.inject_wheel(cx + 31.0, cy + 21.0, -2.0, false);
    assert!(events.borrow().contains(&Event::Wheel {
        x: 31.0,
        y: 21.0,
        wx: 0.0,
        wy: -2.0,
        ctrl: false
    }));
    events.borrow_mut().clear();
    win.inject_mouse(cx + 40.0, cy + 25.0, None);
    win.inject_mouse(cx + 40.0, cy + 25.0, Some(true));
    win.inject_mouse(cx + 40.0, cy + 25.0, Some(false));
    let seen = events.borrow().clone();
    assert!(
        seen.iter()
            .any(|e| matches!(e, Event::MouseMove { x, y, .. } if (*x, *y) == (40.0, 25.0))),
        "{seen:?}"
    );
    assert!(
        seen.contains(&Event::MouseDown {
            x: 40.0,
            y: 25.0,
            button: MouseButton::Left,
            clicks: 1
        }),
        "{seen:?}"
    );
    assert!(
        seen.contains(&Event::MouseUp {
            x: 40.0,
            y: 25.0,
            button: MouseButton::Left
        }),
        "{seen:?}"
    );
    // A custom widget made in code: it is painted once it has a parent in a window, and fills
    // an Overlay.
    let made = custom::create();
    let made_paints: Rc<RefCell<Vec<(f32, f32)>>> = Rc::default();
    custom::on_paint(made, {
        let made_paints = made_paints.clone();
        move |p, w, h| {
            p.rect(0.0, 0.0, 1.0, 1.0, 0.0, Rgba(1.0, 1.0, 1.0, 1.0));
            made_paints.borrow_mut().push((w, h));
        }
    });
    let host = win.find("host").expect("overlay host");
    host.add_child(made);
    made.invalidate();
    assert!(frames_until(5, || !made_paints.borrow().is_empty()));
    let (_, _, hw, hh) = host.rect();
    let (mw, mh) = made_paints.borrow()[0];
    assert!(mw > 0.0 && mh > 0.0 && (mw - hw).abs() < 1.0 && (mh - hh).abs() < 1.0);
    assert_eq!(made.rect(), host.rect());

    // The NULL backend reads back a frame of the window's size (its clear color only).
    let (fw, fh, rgba) = win.capture().expect("a frame from the NULL backend");
    let (ww, wh) = win.size();
    assert_eq!((fw as i32, fh as i32), (ww, wh));
    assert_eq!(rgba.len(), fw as usize * fh as usize * 4);

    // Lists: a virtual list asks for the rows it draws; a click selects the first row.
    let files = win.find("files").expect("list view");
    let asked = Rc::new(Cell::new(0usize));
    let selected_rows: Rc<RefCell<Vec<Option<usize>>>> = Rc::default();
    list::on_selection(files, {
        let selected_rows = selected_rows.clone();
        move |i| selected_rows.borrow_mut().push(i)
    });
    list::set_virtual(files, 1000, {
        let asked = asked.clone();
        move |i| {
            asked.set(asked.get() + 1);
            format!("file {i}")
        }
    });
    assert!(
        frames_until(5, || asked.get() > 0),
        "the list asked for no row"
    );
    // Setting the rows again replaces the previous closure.
    list::set_virtual(files, 3, |i| format!("row {i}"));
    draw_frame();
    let (lx, ly, _, _) = files.rect();
    click(win, lx + 20.0, ly + 6.0);
    draw_frame();
    assert_eq!(list::selected(files), Some(0));
    assert_eq!(selected_rows.borrow().last(), Some(&Some(0)));
    list::set_selected(files, Some(2));
    assert_eq!(list::selected(files), Some(2));
    list::set_selected(files, None);
    assert_eq!(list::selected(files), None);

    // Tree.
    let folders = win.find("folders").expect("tree view");
    let picked: Rc<RefCell<Vec<Option<tree::TreeItem>>>> = Rc::default();
    tree::on_selection(folders, {
        let picked = picked.clone();
        move |item| picked.borrow_mut().push(item)
    });
    let src = tree::add(folders, None, "src");
    let main_rs = tree::add(folders, Some(src), "main.rs");
    assert_ne!(src, main_rs);
    draw_frame();
    let (tx, ty, _, _) = folders.rect();
    click(win, tx + 30.0, ty + 6.0);
    draw_frame();
    assert_eq!(picked.borrow().last(), Some(&Some(src)));
    // Branches open and close from code (what shows is checked by eye in the app).
    tree::set_expanded(src, true);
    draw_frame();
    tree::set_expanded(src, false);
    draw_frame();
    tree::clear(folders);
    draw_frame();

    // Combo box.
    let choice = win.find("choice").expect("combo box");
    let combo_changes: Rc<RefCell<Vec<usize>>> = Rc::default();
    combo::on_changed(choice, {
        let combo_changes = combo_changes.clone();
        move |i| combo_changes.borrow_mut().push(i)
    });
    combo::set_items(choice, &["Haiku", "Sonnet", "Opus"]);
    combo::set_selected(choice, 2);
    assert_eq!(combo::selected(choice), Some(2));
    combo::set_items(choice, &["Only"]);
    combo::set_selected(choice, 0);
    assert_eq!(combo::selected(choice), Some(0));

    // Tabs.
    let pages = win.find("pages").expect("tab view");
    let tab_changes: Rc<RefCell<Vec<usize>>> = Rc::default();
    tabs::on_changed(pages, {
        let tab_changes = tab_changes.clone();
        move |i| tab_changes.borrow_mut().push(i)
    });
    tabs::on_close(pages, |_content| true);
    let one = widget_from_xml(r#"<Label Text="one"/>"#).expect("tab one");
    let two = widget_from_xml(r#"<Label Text="two"/>"#).expect("tab two");
    assert_eq!(tabs::add(pages, "One", one), 0);
    assert_eq!(tabs::add(pages, "Two", two), 1);
    tabs::set_title(pages, two, "Second");
    tabs::set_closable(pages, one, false);
    tabs::set_active(pages, 1);
    draw_frame();
    tabs::set_active(pages, 0);
    draw_frame();
    // A page holds widgets that only it can find, even before it is in a window; destroying
    // the page takes them with it.
    let three =
        widget_from_xml(r#"<VBox><Label Name="inside" Text="three"/></VBox>"#).expect("tab three");
    let inside = three.find("inside").expect("found inside its own tree");
    assert_eq!(win.find("inside"), None, "not in the window yet");
    assert_eq!(tabs::add(pages, "Three", three), 2);
    assert_eq!(three.find("inside"), Some(inside));
    assert_eq!(
        win.find("inside"),
        Some(inside),
        "in the window with its page"
    );
    assert_eq!(three.find("nope"), None);
    three.destroy();
    draw_frame();
    assert_eq!(win.find("inside"), None, "gone with its page");
    assert_eq!(
        tabs::add(pages, "Four", widget_from_xml("<Label/>").expect("four")),
        2
    );

    // Dialogs: open with content, close with a result.
    let content = widget_from_xml(
        r#"<VBox Width="240"><Label Text="Sure?"/><Button Name="ok" Text="OK"/></VBox>"#,
    )
    .expect("dialog content");
    let closed_with = Rc::new(Cell::new(None));
    dialog::open(win, content, {
        let closed_with = closed_with.clone();
        move |result| closed_with.set(Some(result))
    });
    draw_frame();
    dialog::close(content, 7);
    assert!(frames_until(3, || closed_with.get().is_some()));
    assert_eq!(closed_with.get(), Some(7));

    // Theme.
    theme::load_xml(r##"<Theme Name="Smoke" Base="Dark" Accent="#38BDF8"/>"##).expect("theme");
    assert!(theme::load_xml("this is not a theme").is_err());
    theme::set_dark(true);
    theme::set_accent(0.22, 0.74, 0.97);
    draw_frame();

    // Maximizing is the last window change: the window keeps its maximized size after it.
    win.maximize();
    draw_frame();

    // The window stayed active the whole time under the NULL platform, and nobody asked to
    // close it.
    assert_eq!(focus_gained.get(), 0);
    assert_eq!(closes.get(), 0);

    // Waiting dialogs, last: they run frames while they wait, so a timer answers them by quitting.
    // A dismissed question without a Cancel button is answered "no".
    let _ = task::timer(0.01, false, || quit(0));
    assert_eq!(
        dialog::message(Some(win), "Title", "Question", dialog::Buttons::YesNo),
        dialog::Answer::No
    );
    assert_eq!(dialog::select_folder("Pick a folder"), None);
    assert!(!draw_frame(), "quit stops the frame loop");

    shutdown();
}
