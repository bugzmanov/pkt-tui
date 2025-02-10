# PKT-TUI - A Terminal Interface for GetPocket

A feature-rich terminal user interface for managing your Pocket reading list with style and efficiency. Built with Rust using `ratatui` and offering a seamless terminal-based reading list management experience.

<img src="https://github.com/user-attachments/assets/3bac3e90-ff27-4ef5-aeb2-43e5a2f00b89"/>

## ✨ Features

### 📚 Content Management
- View and manage your entire Pocket reading list from the terminal
- Smart content type detection (articles, videos, PDFs)
- Efficient tag management and filtering
- Quick access to favorite and top-rated items
- Bulk operations for efficient list management

### 🔍 Advanced Filtering
- Filter by content type (articles, videos, PDFs)
- Tag-based filtering with an interactive tag browser
- Full-text search across titles and URLs
- Domain/author filtering with statistics
- Multiple active filters support

### 📊 Reading Stats
- Track your reading habits with detailed statistics
- Daily, weekly, and monthly reading summaries
- Content type distribution analysis
- Domain and author analytics

### 🎨 User Interface
- Clean, modern terminal interface with custom styling
- Vim-style keybindings for navigation
- Interactive scrolling and mouse support
- Customizable color schemes
- Responsive layout adapting to terminal size

### 🚀 Performance
- Fast and responsive even with large reading lists
- Efficient data synchronization with Pocket API
- Local caching for quick startup
- Delta updates to minimize data transfer

## 🛠 Installation

```bash
# Clone the repository
git clone https://github.com/bugzmanov/pkt-tui
cd pkt-tui

# Build and install
cargo install --path .
```

## 📝 Configuration

On first run, the application will guide you through the authentication process with Pocket. Your authentication token will be securely stored for future use.

## ⌨️ Key Bindings

### Navigation
- `j/k` or `↑/↓` - Navigate items
- `Ctrl+d/u` - Page down/up
- `gg` - Jump to start
- `G` - Jump to end
- `gd` - Jump to date

### Actions
- `Enter` - Open selected item in browser
- `z` - Show tag browser
- `t` - Toggle top tag
- `f` - Favorite and archive
- `d` - Delete item
- `r` - Rename item
- `w` - Download PDF (for PDF items)
- `s` - Filter by current domain/author
- `S` - Show domain statistics
- `i` - Filter by document type
- `?` - Show help

### Filtering
- `/` - Search mode
- `Esc` - Clear current filter
- `Q` - Refresh data from Pocket

## 🤝 Contributing

Contributions are welcome!

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

Built with ❤️ using Rust and [ratatui](https://github.com/ratatui/ratatui)
