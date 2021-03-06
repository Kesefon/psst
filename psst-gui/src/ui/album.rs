use crate::{
    cmd,
    data::{Album, AlbumDetail, Artist, CommonCtx, Ctx, Nav, State},
    ui::{
        theme,
        track::{make_tracklist, TrackDisplay},
        utils::{make_error, make_loader, make_placeholder},
    },
    widget::{Async, HoverExt, RemoteImage},
};
use druid::{
    widget::{CrossAxisAlignment, Flex, Label, LineBreaking, List},
    ContextMenu, LensExt, LocalizedString, MenuDesc, MenuItem, MouseButton, Widget, WidgetExt,
};

pub fn make_detail() -> impl Widget<State> {
    Async::new(
        || make_loader(),
        || make_detail_loaded(),
        || make_error().lens(Ctx::data()),
    )
    .lens(
        Ctx::make(State::common_ctx, State::album.then(AlbumDetail::album)).then(Ctx::in_promise()),
    )
}

fn make_detail_loaded() -> impl Widget<Ctx<CommonCtx, Album>> {
    let album_cover = make_cover(theme::grid(30.0), theme::grid(30.0));

    let album_name = Label::raw()
        .with_line_break_mode(LineBreaking::WordWrap)
        .with_font(theme::UI_FONT_MEDIUM)
        .lens(Album::name);

    let album_artists = List::new(|| {
        Label::raw()
            .with_line_break_mode(LineBreaking::WordWrap)
            .hover()
            .lens(Artist::name)
            .on_click(|ctx, artist: &mut Artist, _| {
                let nav = Nav::ArtistDetail(artist.link());
                ctx.submit_command(cmd::NAVIGATE_TO.with(nav));
            })
    })
    .lens(Album::artists);

    let album_date = Label::dynamic(|album: &Album, _| album.release());

    let album_label = Label::raw()
        .with_line_break_mode(LineBreaking::WordWrap)
        .with_text_size(theme::TEXT_SIZE_SMALL)
        .with_text_color(theme::PLACEHOLDER_COLOR)
        .lens(Album::label);

    let album_copyrights = List::new(|| {
        Label::raw()
            .with_line_break_mode(LineBreaking::WordWrap)
            .with_text_size(theme::TEXT_SIZE_SMALL)
            .with_text_color(theme::PLACEHOLDER_COLOR)
    })
    .lens(Album::copyrights);

    let album_info = Flex::column()
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(album_cover)
        .with_spacer(theme::grid(2.0))
        .with_child(album_name)
        .with_spacer(theme::grid(0.5))
        .with_child(album_artists)
        .with_spacer(theme::grid(0.5))
        .with_child(album_date)
        .with_default_spacer()
        .with_child(album_label)
        .with_spacer(theme::grid(0.5))
        .with_child(album_copyrights)
        .fix_width(theme::grid(30.0))
        .lens(Ctx::data());

    let album_tracks = make_tracklist(TrackDisplay {
        number: true,
        title: true,
        artist: false,
        album: false,
    });

    Flex::row()
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(album_info)
        .with_default_spacer()
        .with_flex_child(album_tracks, 1.0)
        .padding((theme::grid(1.0), 0.0))
}

pub fn make_cover(width: f64, height: f64) -> impl Widget<Album> {
    RemoteImage::new(make_placeholder(), move |album: &Album, _| {
        album.image(width, height).map(|image| image.url.clone())
    })
    .fix_size(width, height)
}

pub fn make_album() -> impl Widget<Ctx<CommonCtx, Album>> {
    let album_cover = make_cover(theme::grid(7.0), theme::grid(7.0));

    let album_name = Label::raw()
        .with_font(theme::UI_FONT_MEDIUM)
        .with_line_break_mode(LineBreaking::Clip)
        .lens(Album::name);

    let album_artists = Label::dynamic(|album: &Album, _| album.artist_list())
        .with_text_size(theme::TEXT_SIZE_SMALL)
        .with_line_break_mode(LineBreaking::Clip);

    let album_date = Label::dynamic(|album: &Album, _| album.release_year())
        .with_text_size(theme::TEXT_SIZE_SMALL)
        .with_text_color(theme::PLACEHOLDER_COLOR);

    let album_label = Flex::column()
        .cross_axis_alignment(CrossAxisAlignment::Start)
        .with_child(album_name)
        .with_spacer(1.0)
        .with_child(album_artists)
        .with_spacer(1.0)
        .with_child(album_date);

    let album = Flex::row()
        .with_child(album_cover)
        .with_default_spacer()
        .with_flex_child(album_label, 1.0)
        .lens(Ctx::data());

    album
        .hover()
        .on_ex_click(
            move |ctx, event, album: &mut Ctx<CommonCtx, Album>, _| match event.button {
                MouseButton::Left => {
                    let nav = Nav::AlbumDetail(album.data.link());
                    ctx.submit_command(cmd::NAVIGATE_TO.with(nav));
                }
                MouseButton::Right => {
                    let menu = make_album_menu(&album);
                    ctx.show_context_menu(ContextMenu::new(menu, event.window_pos));
                }
                _ => {}
            },
        )
}

fn make_album_menu(album: &Ctx<CommonCtx, Album>) -> MenuDesc<State> {
    let mut menu = MenuDesc::empty();

    for artist in &album.data.artists {
        let more_than_one_artist = album.data.artists.len() > 1;
        let title = if more_than_one_artist {
            LocalizedString::new("menu-item-show-artist-name")
                .with_placeholder(format!("Go To {}", artist.name))
        } else {
            LocalizedString::new("menu-item-show-artist").with_placeholder("Go To Artist")
        };
        menu = menu.append(MenuItem::new(
            title,
            cmd::NAVIGATE_TO.with(Nav::ArtistDetail(artist.link())),
        ));
    }

    menu = menu.append(MenuItem::new(
        LocalizedString::new("menu-item-copy-link").with_placeholder("Copy Link"),
        cmd::COPY.with(album.data.url()),
    ));

    menu = menu.append_separator();

    if album.ctx.is_album_saved(&album.data) {
        menu = menu.append(MenuItem::new(
            LocalizedString::new("menu-item-remove-from-library")
                .with_placeholder("Remove from Library"),
            cmd::UNSAVE_ALBUM.with(album.data.link()),
        ));
    } else {
        menu = menu.append(MenuItem::new(
            LocalizedString::new("menu-item-save-to-library").with_placeholder("Save to Library"),
            cmd::SAVE_ALBUM.with(album.data.clone()),
        ));
    }

    menu
}
