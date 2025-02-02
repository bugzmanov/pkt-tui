pub struct UserTokenStorage;

//todo: impl secure storage
impl UserTokenStorage {
    pub fn get_token() -> anyhow::Result<Option<String>> {
        match std::fs::read_to_string("user.key") {
            Ok(token) => Ok(Some(token)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn store_token(token: &str) -> anyhow::Result<()> {
        std::fs::write("user.key", token)?;
        Ok(())
    }
}
