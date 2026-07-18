use std::path::PathBuf;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tree::Tree;
use crate::viewer::Viewer;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Tree,
    Viewer,
}

pub struct App {
    pub root: PathBuf,
    pub focus: Focus,
    pub tree: Tree,
    pub viewer: Viewer,
    pub should_quit: bool,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        let tree = Tree::new(&root);
        Self {
            root,
            focus: Focus::Tree,
            tree,
            viewer: Viewer::new(),
            should_quit: false,
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                return;
            }
            KeyCode::Char('c') if ctrl => {
                self.should_quit = true;
                return;
            }
            KeyCode::Tab => {
                self.focus = match self.focus {
                    Focus::Tree => Focus::Viewer,
                    Focus::Viewer => Focus::Tree,
                };
                return;
            }
            _ => {}
        }
        match self.focus {
            Focus::Tree => self.on_tree_key(key),
            Focus::Viewer => self.on_viewer_key(key, ctrl),
        }
    }

    fn on_tree_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.tree.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.tree.move_selection(-1),
            KeyCode::Enter => {
                if let Some(path) = self.tree.toggle_or_open() {
                    self.viewer.open(&path, &self.root);
                }
            }
            _ => {}
        }
    }

    fn on_viewer_key(&mut self, key: KeyEvent, ctrl: bool) {
        let half_page = (self.viewer.viewport_height / 2).max(1) as isize;
        match key.code {
            KeyCode::Char('d') if ctrl => self.viewer.scroll_by(half_page),
            KeyCode::Char('u') if ctrl => self.viewer.scroll_by(-half_page),
            KeyCode::Char('j') | KeyCode::Down => self.viewer.scroll_by(1),
            KeyCode::Char('k') | KeyCode::Up => self.viewer.scroll_by(-1),
            _ => {}
        }
    }
}
