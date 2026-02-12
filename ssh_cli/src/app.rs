use eframe::egui;
use egui_extras::{Column, TableBuilder};
use ssh2::Session;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;

use crate::model::{FileEncoding, FileEntry, SortColumn, SortDirection};
use crate::ssh::{connect_session, download_worker, list_files_streaming, search_files_streaming};
use ssh2::Sftp;

struct FileViewerState {
    filename: String,
    raw_content: Vec<u8>,
    decoded_content: String,
    encoding: FileEncoding,
}

// Removed duplicate FileViewerState enum

pub enum AppMessage {
    ConnectionResult(Result<(Arc<Mutex<Session>>, Arc<Mutex<Sftp>>, String), String>), // (session, sftp, path)
    // ListResult removed
    ListStarted(String),
    ListBatch(Vec<FileEntry>),
    ListFinished,
    ListError(String),
    SearchStarted(String),
    DownloadResult(Result<String, String>),
    FileContentResult(Result<(String, Vec<u8>), String>), // (filename, raw_content)
}

pub struct SshApp {
    // Session state
    session: Option<Arc<Mutex<Session>>>,
    sftp: Option<Arc<Mutex<Sftp>>>,
    is_connected: bool,

    // Login Data
    host: String,
    user: String,
    password: String,

    // Favorites
    favorites: Vec<crate::model::FavoriteConnection>,
    favorite_name_input: String,

    // Directory Bookmarks
    directory_bookmarks: Vec<crate::model::DirectoryBookmark>,
    bookmark_name_input: String,

    // File Browser State
    files: Vec<FileEntry>,
    selected_file: Option<FileEntry>,
    current_path: String,
    search_query: String,
    recursive_search: bool,

    // File Viewer State
    viewing_file: Option<FileViewerState>,

    // UI State
    status_msg: String,
    is_loading: bool,
    sort_column: SortColumn,
    sort_direction: SortDirection,

    // Concurrency
    receiver: mpsc::Receiver<AppMessage>,
    sender: mpsc::Sender<AppMessage>,
}

impl SshApp {
    pub fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let mut app = Self {
            session: None,
            sftp: None,
            is_connected: false,
            host: "0.0.0.0:22".to_owned(),
            user: "".to_owned(),
            password: "".to_owned(),
            favorites: Vec::new(),
            favorite_name_input: String::new(),
            directory_bookmarks: Vec::new(),
            bookmark_name_input: String::new(),
            files: Vec::new(),
            selected_file: None,
            current_path: String::new(),
            search_query: String::new(),
            recursive_search: false,
            viewing_file: None,
            status_msg: "Ready to connect.".to_owned(),
            is_loading: false,
            sort_column: SortColumn::None,
            sort_direction: SortDirection::Asc,
            receiver,
            sender,
        };

        println!("App loading favorites...");
        app.favorites = app.load_favorites();
        app.directory_bookmarks = app.load_directory_bookmarks();
        println!("App initialized.");

        // Configure fonts for Japanese support
        app.configure_fonts(&_cc.egui_ctx);

        app
    }

    fn configure_fonts(&self, ctx: &egui::Context) {
        let mut fonts = egui::FontDefinitions::default();

        // Try to load a Japanese font from the system
        // Common path for MS Gothic on Windows
        let font_path = "C:\\Windows\\Fonts\\msgothic.ttc";

        if let Ok(font_data) = std::fs::read(font_path) {
            fonts.font_data.insert(
                "MS Gothic".to_owned(),
                egui::FontData::from_owned(font_data).tweak(egui::FontTweak {
                    scale: 1.2, // Slightly larger for readability
                    ..Default::default()
                }),
            );

            // Add MS Gothic to the priority list for Proportional and Monospace
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                family.push("MS Gothic".to_owned());
            }
            if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                family.push("MS Gothic".to_owned());
            }

            ctx.set_fonts(fonts);
            println!("Loaded Japanese font: {}", font_path);
        } else {
            println!("Failed to load Japanese font from: {}", font_path);
            // Fallback: try different one or just log error?
            // Try Meiryo
            let font_path_meiryo = "C:\\Windows\\Fonts\\meiryo.ttc";
            if let Ok(font_data) = std::fs::read(font_path_meiryo) {
                fonts
                    .font_data
                    .insert("Meiryo".to_owned(), egui::FontData::from_owned(font_data));
                if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
                    family.push("Meiryo".to_owned());
                }
                if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Monospace) {
                    family.push("Meiryo".to_owned());
                }
                ctx.set_fonts(fonts);
                println!("Loaded Japanese font: {}", font_path_meiryo);
            } else {
                println!("Failed to load Meiryo as well.");
            }
        }
    }

    fn connect_ssh(&mut self) {
        if self.is_loading {
            return;
        }

        self.is_loading = true;
        self.status_msg = "Connecting...".to_owned();
        let tx = self.sender.clone();

        let host = self.host.clone();
        let user = self.user.clone();
        let pass = self.password.clone();

        thread::spawn(move || {
            match connect_session(&host, &user, &pass) {
                Ok((sess, sftp, path)) => {
                    let sess_arc = Arc::new(Mutex::new(sess));
                    let sftp_arc = Arc::new(Mutex::new(sftp));
                    let _ = tx.send(AppMessage::ConnectionResult(Ok((
                        sess_arc.clone(),
                        sftp_arc.clone(),
                        path.clone(),
                    ))));
                    // Start listing immediately after connection
                    let _ = list_files_streaming(&sftp_arc, &path, tx);
                }
                Err(e) => {
                    let _ = tx.send(AppMessage::ConnectionResult(Err(e.to_string())));
                }
            }
        });
    }

    fn list_directory(&self, path: String) {
        let sftp_arc = self.sftp.clone();
        let tx = self.sender.clone();

        if let Some(sftp_arc) = sftp_arc {
            thread::spawn(move || {
                if let Err(e) = list_files_streaming(&sftp_arc, &path, tx.clone()) {
                    let _ = tx.send(AppMessage::ListError(e.to_string()));
                }
            });
        }
    }

    fn search_files(&self) {
        let sftp_arc = self.sftp.clone();
        let tx = self.sender.clone();
        let path = self.current_path.clone();
        let query = self.search_query.clone();
        let recursive = self.recursive_search;

        if let Some(sftp_arc) = sftp_arc {
            thread::spawn(move || {
                if let Err(e) =
                    search_files_streaming(&sftp_arc, &path, &query, recursive, tx.clone())
                {
                    let _ = tx.send(AppMessage::ListError(e.to_string()));
                }
            });
        }
    }

    fn trigger_sort(&mut self, column: SortColumn) {
        if self.sort_column == column {
            self.sort_direction = match self.sort_direction {
                SortDirection::Asc => SortDirection::Desc,
                SortDirection::Desc => SortDirection::Asc,
            };
        } else {
            self.sort_column = column;
            self.sort_direction = SortDirection::Asc;
        }
        self.sort_files();
    }

    fn sort_files(&mut self) {
        if self.sort_column == SortColumn::None {
            return;
        }

        self.files.sort_by(|a, b| {
            let ord = match self.sort_column {
                SortColumn::Permission => a.perm.cmp(&b.perm),
                SortColumn::Size => a.size.cmp(&b.size),
                SortColumn::Date => a.date.cmp(&b.date),
                SortColumn::Name => a.name.cmp(&b.name),
                SortColumn::None => std::cmp::Ordering::Equal,
            };

            match self.sort_direction {
                SortDirection::Asc => ord,
                SortDirection::Desc => ord.reverse(),
            }
        });
    }

    fn download_file(&self, file_name: String, local_path: PathBuf) {
        let session_arc = self.session.clone();
        let tx = self.sender.clone();
        let current_path = self.current_path.clone();

        if let Some(session_arc) = session_arc {
            thread::spawn(move || {
                let display_name = file_name.clone();
                let remote_path = if current_path.ends_with('/') {
                    format!("{}{}", current_path, &file_name)
                } else if current_path.is_empty() {
                    file_name
                } else {
                    format!("{}/{}", current_path, &file_name)
                };

                let result = download_worker(session_arc, &remote_path, local_path);
                match result {
                    Ok(_) => {
                        let _ = tx.send(AppMessage::DownloadResult(Ok(format!(
                            "Downloaded {}",
                            display_name
                        ))));
                    }
                    Err(e) => {
                        let _ = tx.send(AppMessage::DownloadResult(Err(e.to_string())));
                    }
                }
            });
        }
    }

    fn show_login(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.heading("SSH Login");
                ui.add_space(20.0);

                // Favorites Selection
                ui.horizontal(|ui| {
                    ui.label("Favorites:");
                    egui::ComboBox::from_id_salt("favorites_combo")
                        .selected_text("Select a favorite...")
                        .show_ui(ui, |ui| {
                            let mut selected = None;
                            for fav in &self.favorites {
                                if ui.selectable_label(false, &fav.name).clicked() {
                                    selected = Some(fav.clone());
                                }
                            }
                            if let Some(fav) = selected {
                                self.host = fav.host;
                                self.user = fav.user;
                                self.password = fav.password;
                            }
                        });
                });
                ui.add_space(10.0);

                egui::Grid::new("login_grid")
                    .num_columns(2)
                    .spacing([10.0, 10.0])
                    .show(ui, |ui| {
                        ui.label("Host (IP:Port):");
                        ui.text_edit_singleline(&mut self.host);
                        ui.end_row();

                        ui.label("Username:");
                        ui.text_edit_singleline(&mut self.user);
                        ui.end_row();

                        ui.label("Password:");
                        ui.add(egui::TextEdit::singleline(&mut self.password).password(true));
                        ui.end_row();
                    });

                ui.add_space(10.0);

                // Save Favorite UI
                ui.horizontal(|ui| {
                    ui.label("Name:");
                    ui.text_edit_singleline(&mut self.favorite_name_input);
                    if ui.button("Save as Favorite").clicked() {
                        self.save_favorite();
                    }
                    if ui.button("Delete Favorite").clicked() {
                        self.delete_favorite();
                    }
                });

                ui.add_space(20.0);
                if self.is_loading {
                    ui.spinner();
                } else {
                    if ui.button("Connect").clicked()
                        || ctx.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        self.connect_ssh();
                    }
                }

                ui.add_space(10.0);
                ui.label(egui::RichText::new(&self.status_msg).color(egui::Color32::RED));
            });
        });
    }

    fn load_favorites(&self) -> Vec<crate::model::FavoriteConnection> {
        if let Ok(file) = std::fs::File::open("favorites.json") {
            if let Ok(favs) = serde_json::from_reader(file) {
                return favs;
            }
        }
        Vec::new()
    }

    fn save_favorites_to_disk(&self) {
        if let Ok(file) = std::fs::File::create("favorites.json") {
            let _ = serde_json::to_writer_pretty(file, &self.favorites);
        }
    }

    fn save_favorite(&mut self) {
        if self.favorite_name_input.is_empty() {
            return;
        }

        // Check if exists and update, or push new
        let new_fav = crate::model::FavoriteConnection {
            name: self.favorite_name_input.clone(),
            host: self.host.clone(),
            user: self.user.clone(),
            password: self.password.clone(),
        };

        if let Some(pos) = self.favorites.iter().position(|f| f.name == new_fav.name) {
            self.favorites[pos] = new_fav;
        } else {
            self.favorites.push(new_fav);
        }
        self.save_favorites_to_disk();
        self.status_msg = format!("Saved favorite '{}'", self.favorite_name_input);
    }

    fn delete_favorite(&mut self) {
        if self.favorite_name_input.is_empty() {
            return;
        }

        if let Some(pos) = self
            .favorites
            .iter()
            .position(|f| f.name == self.favorite_name_input)
        {
            self.favorites.remove(pos);
            self.save_favorites_to_disk();
            self.status_msg = format!("Deleted favorite '{}'", self.favorite_name_input);
            self.favorite_name_input.clear();
        } else {
            self.status_msg = format!("Favorite '{}' not found", self.favorite_name_input);
        }
    }

    fn load_directory_bookmarks(&self) -> Vec<crate::model::DirectoryBookmark> {
        if let Ok(file) = std::fs::File::open("directory_bookmarks.json") {
            if let Ok(bookmarks) = serde_json::from_reader(file) {
                return bookmarks;
            }
        }
        Vec::new()
    }

    fn save_directory_bookmarks(&self) {
        if let Ok(file) = std::fs::File::create("directory_bookmarks.json") {
            let _ = serde_json::to_writer_pretty(file, &self.directory_bookmarks);
        }
    }

    fn add_directory_bookmark(&mut self) {
        if self.bookmark_name_input.is_empty() {
            self.status_msg = "Bookmark name cannot be empty.".to_owned();
            return;
        }

        let new_bookmark = crate::model::DirectoryBookmark {
            name: self.bookmark_name_input.clone(),
            path: self.current_path.clone(),
            host: self.host.clone(),
        };

        if let Some(pos) = self.directory_bookmarks.iter().position(|b| b.name == new_bookmark.name) {
            self.directory_bookmarks[pos] = new_bookmark;
            self.status_msg = format!("Updated bookmark '{}'", self.bookmark_name_input);
        } else {
            self.directory_bookmarks.push(new_bookmark);
            self.status_msg = format!("Added bookmark '{}'", self.bookmark_name_input);
        }
        self.save_directory_bookmarks();
        self.bookmark_name_input.clear();
    }

    fn delete_directory_bookmark(&mut self) {
        if self.bookmark_name_input.is_empty() {
            self.status_msg = "Bookmark name cannot be empty.".to_owned();
            return;
        }

        if let Some(pos) = self.directory_bookmarks.iter().position(|b| b.name == self.bookmark_name_input) {
            self.directory_bookmarks.remove(pos);
            self.save_directory_bookmarks();
            self.status_msg = format!("Deleted bookmark '{}'", self.bookmark_name_input);
            self.bookmark_name_input.clear();
        } else {
            self.status_msg = format!("Bookmark '{}' not found", self.bookmark_name_input);
        }
    }

    fn navigate_to_bookmark(&mut self, bookmark_path: String) {
        println!("Navigating to bookmark: {}", bookmark_path);
        self.is_loading = true;
        self.list_directory(bookmark_path);
    }

    fn view_file(&self, file_name: String) {
        let sftp_arc = self.sftp.clone();
        let tx = self.sender.clone();
        let current_path = self.current_path.clone();

        if let Some(sftp_arc) = sftp_arc {
            thread::spawn(move || {
                let remote_path = if current_path.ends_with('/') {
                    format!("{}{}", current_path, &file_name)
                } else if current_path.is_empty() {
                    file_name
                } else {
                    format!("{}/{}", current_path, &file_name)
                };

                // Use SFTP API to read file content (max 100KB)
                if let Err(e) =
                    crate::ssh::read_file_content(&sftp_arc, &remote_path, 100000, tx.clone())
                {
                    let _ = tx.send(AppMessage::FileContentResult(Err(e.to_string())));
                }
            });
        }
    }

    fn show_file_viewer(&mut self, ctx: &egui::Context) {
        let mut is_open = self.viewing_file.is_some();
        if is_open {
            if let Some(state) = &mut self.viewing_file {
                egui::Window::new(format!("Viewing: {}", state.filename))
                    .open(&mut is_open)
                    .default_size([600.0, 400.0])
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("Encoding:");
                            let previous_encoding = state.encoding;

                            egui::ComboBox::from_id_salt("encoding_combo")
                                .selected_text(format!("{}", state.encoding))
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(
                                        &mut state.encoding,
                                        FileEncoding::Utf8,
                                        "UTF-8",
                                    );
                                    ui.selectable_value(
                                        &mut state.encoding,
                                        FileEncoding::ShiftJis,
                                        "Shift-JIS",
                                    );
                                });

                            if state.encoding != previous_encoding {
                                // Re-decode on change
                                let coder = match state.encoding {
                                    FileEncoding::Utf8 => encoding_rs::UTF_8,
                                    FileEncoding::ShiftJis => encoding_rs::SHIFT_JIS,
                                };
                                let (decoded, _, _) = coder.decode(&state.raw_content);
                                state.decoded_content = decoded.into_owned();
                            }
                        });
                        ui.separator();

                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut state.decoded_content)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .code_editor(),
                            );
                        });
                    });
            }
        }
        if !is_open {
            self.viewing_file = None;
        }
    }

    fn show_browser(&mut self, ctx: &egui::Context) {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("SSH File Browser");
                if self.is_loading {
                    ui.spinner();
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Disconnect").clicked() {
                        self.is_connected = false;
                        self.session = None;
                        self.files.clear();
                        self.status_msg = "Disconnected.".to_owned();
                    }
                });
            });

            // Address Bar
            ui.horizontal(|ui| {
                if ui
                    .button("⬆")
                    .on_hover_text("Go to parent directory")
                    .clicked()
                {
                    let mut new_path = self.current_path.trim_end_matches('/').to_string();
                    if !new_path.is_empty() {
                        if let Some(idx) = new_path.rfind('/') {
                            if idx == 0 {
                                new_path = "/".to_string();
                            } else {
                                new_path.truncate(idx);
                            }
                            self.is_loading = true;
                            self.list_directory(new_path);
                        }
                    }
                }

                ui.label("Path:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.current_path).desired_width(f32::INFINITY),
                );
                if response.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.is_loading = true;
                    self.list_directory(self.current_path.clone());
                }

                if ui.button("Go").clicked() {
                    self.is_loading = true;
                    self.list_directory(self.current_path.clone());
                }
            });

            // Bookmarks Bar
            ui.horizontal(|ui| {
                ui.label("Bookmarks:");
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        // Filter bookmarks for current host
                        let current_host_bookmarks: Vec<_> = self.directory_bookmarks
                            .iter()
                            .filter(|b| b.host == self.host)
                            .collect();
                        
                        if current_host_bookmarks.is_empty() {
                            ui.label("(No bookmarks for this host)");
                        } else {
                            let mut path_to_navigate: Option<String> = None;
                            for bookmark in current_host_bookmarks {
                                if ui.button(&bookmark.name).clicked() {
                                    println!("Bookmark clicked: {} -> {}", bookmark.name, bookmark.path);
                                    path_to_navigate = Some(bookmark.path.clone());
                                    break; // Only handle one click per frame
                                }
                            }
                            // Navigate after the loop to avoid borrow checker issues
                            if let Some(path) = path_to_navigate {
                                self.navigate_to_bookmark(path);
                            }
                        }
                    });
                });
            });

            // Bookmark Management
            ui.horizontal(|ui| {
                ui.label("Bookmark:");
                ui.text_edit_singleline(&mut self.bookmark_name_input);
                if ui.button("Add").clicked() {
                    self.add_directory_bookmark();
                }
                if ui.button("Delete").clicked() {
                    self.delete_directory_bookmark();
                }
            });

            // Search Bar
            ui.horizontal(|ui| {
                ui.label("Search:");
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.search_query)
                        .hint_text("Filename pattern (e.g. *.txt)"),
                );
                ui.checkbox(&mut self.recursive_search, "Recursive");

                if ui.button("Search").clicked()
                    || (response.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)))
                {
                    if !self.search_query.is_empty() {
                        self.is_loading = true;
                        self.search_files();
                    }
                }
            });

            ui.label(&self.status_msg);
            ui.separator();

            // Action Bar
            ui.horizontal(|ui| {
                if ui.button("Refresh").clicked() {
                    self.is_loading = true;
                    self.list_directory(self.current_path.clone());
                }

                if let Some(file) = &self.selected_file {
                    if ui.button("View").clicked() {
                        self.is_loading = true;
                        self.status_msg = format!("Reading {}...", file.name);
                        self.view_file(file.name.clone());
                    }
                    if ui.button("Download").clicked() {
                        if let Some(path) =
                            rfd::FileDialog::new().set_file_name(&file.name).save_file()
                        {
                            self.is_loading = true;
                            self.status_msg = format!("Downloading {}...", file.name);
                            self.download_file(file.name.clone(), path);
                        }
                    }
                }
            });

            ui.separator();

            // File Table
            let text_height = egui::TextStyle::Body.resolve(ui.style()).size + 5.0; // slightly taller rows

            TableBuilder::new(ui)
                .striped(true)
                .resizable(true)
                .cell_layout(egui::Layout::left_to_right(egui::Align::Center))
                .column(Column::auto())
                .column(Column::auto())
                .column(Column::auto())
                .column(Column::remainder())
                .header(20.0, |mut header| {
                    header.col(|ui| {
                        if ui.button("Permissions").clicked() {
                            self.trigger_sort(SortColumn::Permission);
                        }
                    });
                    header.col(|ui| {
                        if ui.button("Size").clicked() {
                            self.trigger_sort(SortColumn::Size);
                        }
                    });
                    header.col(|ui| {
                        if ui.button("Date").clicked() {
                            self.trigger_sort(SortColumn::Date);
                        }
                    });
                    header.col(|ui| {
                        if ui.button("Name").clicked() {
                            self.trigger_sort(SortColumn::Name);
                        }
                    });
                })
                .body(|body| {
                    body.rows(text_height, self.files.len(), |mut row| {
                        let row_index = row.index();
                        let file = self.files[row_index].clone();
                        let is_selected = self
                            .selected_file
                            .as_ref()
                            .map_or(false, |f| f.name == file.name);

                        row.col(|ui| {
                            ui.label(&file.perm);
                        });
                        row.col(|ui| {
                            ui.label(file.size.to_string());
                        });
                        row.col(|ui| {
                            ui.label(&file.date);
                        });
                        row.col(|ui| {
                            let label = ui.selectable_label(is_selected, &file.name);
                            if label.clicked() {
                                self.selected_file = Some(file.clone());
                            }
                            if label.double_clicked() {
                                // Navigate if directory?
                                // ls -l perms start with 'd' for directory
                                if file.perm.starts_with('d') {
                                    let new_path = if self.current_path.ends_with('/') {
                                        format!("{}{}", self.current_path, &file.name)
                                    } else if self.current_path.is_empty() {
                                        file.name.clone()
                                    } else {
                                        format!("{}/{}", self.current_path, &file.name)
                                    };
                                    self.is_loading = true;
                                    self.list_directory(new_path);
                                }
                            }
                        });
                    });
                });
        });
    }
}

impl eframe::App for SshApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        while let Ok(msg) = self.receiver.try_recv() {
            match msg {
                AppMessage::ConnectionResult(res) => {
                    match res {
                        Ok((sess_arc, sftp_arc, path)) => {
                            self.session = Some(sess_arc);
                            self.sftp = Some(sftp_arc);
                            self.current_path = path;
                            self.status_msg = "Connected.".to_owned();
                            self.is_connected = true;
                            self.is_loading = false; // Stop spinner
                        }
                        Err(e) => {
                            self.is_loading = false;
                            self.status_msg = format!("Error: {}", e);
                            self.is_connected = false;
                        }
                    }
                }
                AppMessage::ListStarted(path) => {
                    self.is_loading = true;
                    self.files.clear();
                    self.selected_file = None;
                    self.current_path = path;
                    self.status_msg = "Listing files...".to_owned();
                }
                AppMessage::SearchStarted(query) => {
                    self.is_loading = true;
                    self.files.clear();
                    self.selected_file = None;
                    self.status_msg = format!("Searching for '{}'...", query);
                }
                AppMessage::ListBatch(mut batch) => {
                    self.files.append(&mut batch);
                    if self.sort_column != SortColumn::None {
                        self.sort_files();
                    }
                }
                AppMessage::ListFinished => {
                    self.is_loading = false;
                    self.status_msg = format!("Listed {} files.", self.files.len());
                    // Apply sort if active
                    if self.sort_column != SortColumn::None {
                        self.sort_files();
                    }
                }
                AppMessage::ListError(e) => {
                    self.is_loading = false;
                    self.status_msg = format!("List error: {}", e);
                }
                AppMessage::DownloadResult(res) => {
                    self.is_loading = false;
                    match res {
                        Ok(msg) => self.status_msg = msg,
                        Err(e) => self.status_msg = format!("Download failed: {}", e),
                    }
                }
                AppMessage::FileContentResult(res) => {
                    self.is_loading = false;
                    match res {
                        Ok((name, raw_content)) => {
                            // Default to UTF-8
                            let decoded_string =
                                encoding_rs::UTF_8.decode(&raw_content).0.into_owned();
                            self.viewing_file = Some(FileViewerState {
                                filename: name,
                                raw_content,
                                decoded_content: decoded_string,
                                encoding: FileEncoding::Utf8,
                            });
                            self.status_msg = "File content loaded.".to_owned();
                        }
                        Err(e) => {
                            self.status_msg = format!("Failed to read file: {}", e);
                        }
                    }
                }
            }
        }

        if !self.is_connected {
            self.show_login(ctx);
        } else {
            self.show_browser(ctx);
            // Show file viewer modal if active
            if self.viewing_file.is_some() {
                self.show_file_viewer(ctx);
            }
        }
    }
}

impl SshApp {
    // ... (rest of impl)
}
