use std::path::Path;

/// Recursively creates a directory tree synchronously, ensuring `0700` (user-only read/write/execute) permissions on Unix.
pub fn create_dir_all_sync_with_permissions<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)
    }
}

/// Recursively creates a directory tree asynchronously, ensuring `0700` (user-only read/write/execute) permissions on Unix.
pub async fn create_dir_all_async_with_permissions<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        let mut builder = tokio::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700);
        builder.create(path).await
    }
    #[cfg(not(unix))]
    {
        tokio::fs::create_dir_all(path).await
    }
}

/// Writes file contents asynchronously, ensuring `0600` (user-only read/write) permissions on Unix.
pub async fn write_file_async_with_permissions<P: AsRef<Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> std::io::Result<()> {
    tokio::fs::write(&path, contents).await?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Writes file contents synchronously, ensuring `0600` (user-only read/write) permissions on Unix.
pub fn write_file_sync_with_permissions<P: AsRef<Path>, C: AsRef<[u8]>>(
    path: P,
    contents: C,
) -> std::io::Result<()> {
    std::fs::write(&path, contents)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

/// Explicitly sets a file's permissions to `0600` (user-only read/write) on Unix.
pub fn set_file_permissions_to_user_only<P: AsRef<Path>>(path: P) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_dir_all_sync_with_permissions() {
        let temp_dir = tempdir().unwrap();
        let nested_path = temp_dir.path().join("nested_dir_sync/deep_dir");

        create_dir_all_sync_with_permissions(&nested_path).unwrap();

        assert!(nested_path.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&nested_path).unwrap();
            let mode = metadata.mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[tokio::test]
    async fn test_create_dir_all_async_with_permissions() {
        let temp_dir = tempdir().unwrap();
        let nested_path = temp_dir.path().join("nested_dir_async/deep_dir");

        create_dir_all_async_with_permissions(&nested_path)
            .await
            .unwrap();

        assert!(nested_path.is_dir());

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&nested_path).unwrap();
            let mode = metadata.mode();
            assert_eq!(mode & 0o777, 0o700);
        }
    }

    #[test]
    fn test_write_file_sync_with_permissions() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("sync_file.txt");
        let content = b"Hello, Sync Permissions!";

        write_file_sync_with_permissions(&file_path, content).unwrap();

        assert!(file_path.is_file());
        let read_content = std::fs::read(&file_path).unwrap();
        assert_eq!(read_content, content);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&file_path).unwrap();
            let mode = metadata.mode();
            // The file permissions should be 0o600 (S_IRUSR | S_IWUSR)
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[tokio::test]
    async fn test_write_file_async_with_permissions() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("async_file.txt");
        let content = b"Hello, Async Permissions!";

        write_file_async_with_permissions(&file_path, content)
            .await
            .unwrap();

        assert!(file_path.is_file());
        let read_content = tokio::fs::read(&file_path).await.unwrap();
        assert_eq!(read_content, content);

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&file_path).unwrap();
            let mode = metadata.mode();
            // The file permissions should be 0o600 (S_IRUSR | S_IWUSR)
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn test_set_file_permissions_to_user_only() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("perm_file.txt");
        std::fs::write(&file_path, b"content").unwrap();

        set_file_permissions_to_user_only(&file_path).unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let metadata = std::fs::metadata(&file_path).unwrap();
            let mode = metadata.mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
