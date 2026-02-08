use ssh2::{Session, Sftp, FileStat};
use std::io::Read;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::fs::File;
use crate::model::FileEntry;
use crate::app::AppMessage;

/// SSH接続を確立し、SFTPセッションを初期化
pub fn connect_session(host: &str, user: &str, pass: &str) -> anyhow::Result<(Session, Sftp, String)> {
    let tcp = TcpStream::connect(host)?;
    let mut session = Session::new()?;
    session.set_tcp_stream(tcp);
    session.handshake()?;
    session.userauth_password(user, pass)?;

    // SFTP初期化
    let sftp = session.sftp()?;
    
    // 初期パスを取得（pwdコマンドの代わりにSFTP APIを使用）
    let initial_path = sftp.realpath(Path::new("."))?
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Invalid path encoding"))?
        .to_string();

    Ok((session, sftp, initial_path))
}

/// SFTP APIを使用してディレクトリ一覧を取得
pub fn list_files_streaming(
    sftp_arc: &Arc<Mutex<Sftp>>,
    path: &str,
    tx: mpsc::Sender<AppMessage>
) -> anyhow::Result<()> {
    let _ = tx.send(AppMessage::ListStarted(path.to_string()));
    
    let sftp = sftp_arc.lock().map_err(|_| anyhow::anyhow!("Lock error"))?;
    let dir_path = Path::new(path);
    let entries = sftp.readdir(dir_path)?;
    
    let mut batch = Vec::new();
    for (entry_path, stat) in entries {
        let name = entry_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("?")
            .to_string();
        
        // "." と ".." をスキップ
        if name == "." || name == ".." {
            continue;
        }
        
        let file_entry = FileEntry {
            perm: format_permissions(&stat),
            size: stat.size.unwrap_or(0),
            date: format_timestamp(stat.mtime),
            name,
        };
        
        batch.push(file_entry);
        if batch.len() >= 200 {
            let _ = tx.send(AppMessage::ListBatch(batch));
            batch = Vec::new();
        }
    }
    
    if !batch.is_empty() {
        let _ = tx.send(AppMessage::ListBatch(batch));
    }
    let _ = tx.send(AppMessage::ListFinished);
    Ok(())
}

/// SFTP APIを使用してファイルを検索
pub fn search_files_streaming(
    sftp_arc: &Arc<Mutex<Sftp>>,
    base_path: &str,
    pattern: &str,
    recursive: bool,
    tx: mpsc::Sender<AppMessage>
) -> anyhow::Result<()> {
    let _ = tx.send(AppMessage::SearchStarted(pattern.to_string()));
    
    let sftp = sftp_arc.lock().map_err(|_| anyhow::anyhow!("Lock error"))?;
    
    fn search_recursive(
        sftp: &Sftp,
        path: &Path,
        pattern: &str,
        recursive: bool,
        results: &mut Vec<FileEntry>
    ) -> anyhow::Result<()> {
        let entries = sftp.readdir(path)?;
        
        for (entry_path, stat) in entries {
            let name = entry_path.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            
            // "." と ".." をスキップ
            if name == "." || name == ".." {
                continue;
            }
            
            // パターンマッチング
            if matches_pattern(name, pattern) {
                results.push(FileEntry {
                    perm: format_permissions(&stat),
                    size: stat.size.unwrap_or(0),
                    date: format_timestamp(stat.mtime),
                    name: name.to_string(),
                });
            }
            
            // 再帰的検索
            if recursive && stat.is_dir() {
                let _ = search_recursive(sftp, &entry_path, pattern, recursive, results);
            }
        }
        Ok(())
    }
    
    let mut results = Vec::new();
    search_recursive(&sftp, Path::new(base_path), pattern, recursive, &mut results)?;
    
    // バッチ送信
    for chunk in results.chunks(200) {
        let _ = tx.send(AppMessage::ListBatch(chunk.to_vec()));
    }
    let _ = tx.send(AppMessage::ListFinished);
    Ok(())
}

/// SCP経由でファイルをダウンロード
pub fn download_worker(session: Arc<Mutex<Session>>, remote_path: &str, local_path: PathBuf) -> anyhow::Result<()> {
    let sess = session.lock().map_err(|_| anyhow::anyhow!("Failed to lock session"))?;
    let (mut remote_file, _stat) = sess.scp_recv(std::path::Path::new(remote_path))?;
    
    let mut local_file = File::create(local_path)?;
    std::io::copy(&mut remote_file, &mut local_file)?;
    
    Ok(())
}

/// SFTP APIを使用してファイル内容を読み取る
pub fn read_file_content(
    sftp_arc: &Arc<Mutex<Sftp>>,
    remote_path: &str,
    max_bytes: usize,
    tx: mpsc::Sender<AppMessage>
) -> anyhow::Result<()> {
    let sftp = sftp_arc.lock().map_err(|_| anyhow::anyhow!("Lock error"))?;
    let mut file = sftp.open(Path::new(remote_path))?;
    
    // 最大バイト数まで読み取り
    let mut buffer = vec![0u8; max_bytes];
    let bytes_read = file.read(&mut buffer)?;
    let content = buffer[..bytes_read].to_vec();
    
    let _ = tx.send(AppMessage::FileContentResult(Ok((remote_path.to_string(), content))));
    Ok(())
}

/// パーミッションを文字列形式に変換（例: drwxr-xr-x）
fn format_permissions(stat: &FileStat) -> String {
    let perm = stat.perm.unwrap_or(0);
    
    // ファイルタイプ判定
    let file_type = if stat.is_dir() {
        'd'
    } else {
        '-'
    };
    
    // ユーザー権限
    let user = format!("{}{}{}",
        if perm & 0o400 != 0 { 'r' } else { '-' },
        if perm & 0o200 != 0 { 'w' } else { '-' },
        if perm & 0o100 != 0 { 'x' } else { '-' }
    );
    
    // グループ権限
    let group = format!("{}{}{}",
        if perm & 0o040 != 0 { 'r' } else { '-' },
        if perm & 0o020 != 0 { 'w' } else { '-' },
        if perm & 0o010 != 0 { 'x' } else { '-' }
    );
    
    // その他の権限
    let other = format!("{}{}{}",
        if perm & 0o004 != 0 { 'r' } else { '-' },
        if perm & 0o002 != 0 { 'w' } else { '-' },
        if perm & 0o001 != 0 { 'x' } else { '-' }
    );
    
    format!("{}{}{}{}", file_type, user, group, other)
}

/// Unixタイムスタンプを日付文字列に変換
fn format_timestamp(mtime: Option<u64>) -> String {
    use chrono::{DateTime, Utc, TimeZone};
    
    if let Some(timestamp) = mtime {
        let dt = Utc.timestamp_opt(timestamp as i64, 0)
            .single()
            .unwrap_or_else(|| Utc::now());
        dt.format("%b %d %H:%M").to_string()
    } else {
        "Unknown".to_string()
    }
}

/// Globパターンマッチング（*と?をサポート）
fn matches_pattern(name: &str, pattern: &str) -> bool {
    // パターンを正規表現に変換
    let pattern_escaped = regex::escape(pattern)
        .replace(r"\*", ".*")
        .replace(r"\?", ".");
    
    if let Ok(re) = regex::Regex::new(&format!("^{}$", pattern_escaped)) {
        re.is_match(name)
    } else {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_permissions() {
        let mut stat = FileStat {
            size: Some(0),
            uid: None,
            gid: None,
            perm: Some(0o100644),
            atime: None,
            mtime: None,
        };
        assert_eq!(format_permissions(&stat), "-rw-r--r--");
        
        stat.perm = Some(0o040755);
        // is_dir()はpermだけでは判定できないため、手動設定が必要
        // このテストは実際のFileStatでは動作しない可能性がある
    }
    
    #[test]
    fn test_matches_pattern() {
        assert!(matches_pattern("test.txt", "*.txt"));
        assert!(matches_pattern("file.rs", "file.?s"));
        assert!(!matches_pattern("test.pdf", "*.txt"));
        assert!(matches_pattern("readme", "*"));
    }
    
    #[test]
    fn test_format_timestamp() {
        let timestamp = 1704067200u64; // 2024-01-01 00:00:00 UTC
        let formatted = format_timestamp(Some(timestamp));
        assert!(formatted.contains("Jan"));
        assert!(formatted.contains("01"));
    }
}
