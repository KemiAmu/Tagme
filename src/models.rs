use axum::http::StatusCode;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet};

pub struct AppState {
    pub db: sled::Db, // @user & #topic, 非自描述（不含存储键）
    pub oauth_client_id: String,
    pub oauth_client_secrets: String,
}

#[derive(Serialize, Deserialize)]
pub struct Top(pub Vec<String>);

impl Default for Top {
    fn default() -> Self {
        Top(Vec::new())
    }
}

#[derive(Serialize, Deserialize)]
pub struct Topic {
    pub author: u64,
    pub description: String,
    pub tags: HashMap<String, u32>,
    pub pending_tags: HashSet<String>,
}

#[derive(Serialize, Deserialize)]
pub struct UserData {
    pub topics: Vec<String>,
    // GitHub REST API
    pub access_token: String,
    pub login: String,
    pub name: String,
    pub avatar_url: String,
    pub bio: String,
}

#[derive(Serialize)]
pub struct UserInfo {
    id: String,
    topics: Vec<String>,
    login: String,
    name: String,
    avatar_url: String,
    bio: String,
    status: Option<&'static str>,
}

#[derive(Serialize, Deserialize)]
pub enum UserStatus {
    Normal(UserData),
    Admin(UserData),
    Banned(UserData),
}

impl Default for UserStatus {
    fn default() -> Self {
        Self::Normal(UserData {
            topics: Vec::new(),
            access_token: String::new(),
            login: String::new(),
            name: String::new(),
            avatar_url: String::new(),
            bio: String::new(),
        })
    }
}

impl UserStatus {
    pub fn into_info(self, uid: u64) -> UserInfo {
        let (status, user) = match self {
            UserStatus::Normal(data) => (None, data),
            UserStatus::Admin(data) => (Some("Admin"), data),
            UserStatus::Banned(data) => (Some("Banned"), data),
        };
        UserInfo {
            id: uid.to_string(),
            topics: user.topics,
            login: user.login,
            name: user.name,
            avatar_url: user.avatar_url,
            bio: user.bio,
            status,
        }
    }

    pub fn data(&self) -> &UserData {
        match self {
            Self::Normal(data) | Self::Admin(data) | Self::Banned(data) => data,
        }
    }

    pub fn data_mut(&mut self) -> &mut UserData {
        match self {
            Self::Normal(data) | Self::Admin(data) | Self::Banned(data) => data,
        }
    }

    pub fn into_data(self) -> UserData {
        match self {
            Self::Normal(data) | Self::Admin(data) | Self::Banned(data) => data,
        }
    }

    pub fn is_admin(&self) -> bool {
        matches!(self, Self::Admin(_))
    }

    pub fn is_banned(&self) -> bool {
        matches!(self, Self::Banned(_))
    }

    pub fn as_active(&self) -> Result<(), (StatusCode, &'static str)> {
        match self.is_banned() {
            true => Err((
                StatusCode::FORBIDDEN,
                "Attempted to request an invalid user",
            )),
            false => Ok(()),
        }
    }

    pub fn active_data(&self) -> Result<&UserData, (StatusCode, &'static str)> {
        self.as_active()?;
        Ok(self.data())
    }

    pub fn active_data_mut(&mut self) -> Result<&mut UserData, (StatusCode, &'static str)> {
        self.as_active()?;
        Ok(self.data_mut())
    }

    pub fn into_active_data(self) -> Result<UserData, (StatusCode, &'static str)> {
        self.as_active()?;
        Ok(self.into_data())
    }

    pub fn as_authorized(&self, uid: u64, author: u64) -> Result<(), (StatusCode, &'static str)> {
        match uid == author || self.is_admin() {
            true => Ok(()),
            false => Err((StatusCode::FORBIDDEN, "Access to the resource is denied")),
        }
    }

    #[deprecated = "Use verified_data instead"]
    pub fn authorized_data(
        &self,
        uid: u64,
        author: u64,
    ) -> Result<&UserData, (StatusCode, &'static str)> {
        self.as_authorized(uid, author)?;
        Ok(self.data())
    }

    #[deprecated = "Use verified_data_mut instead"]
    pub fn authorized_data_mut(
        &mut self,
        uid: u64,
        author: u64,
    ) -> Result<&mut UserData, (StatusCode, &'static str)> {
        self.as_authorized(uid, author)?;
        Ok(self.data_mut())
    }

    #[deprecated = "Use into_verified_data instead"]
    pub fn into_authorized_data(
        self,
        uid: u64,
        author: u64,
    ) -> Result<UserData, (StatusCode, &'static str)> {
        self.as_authorized(uid, author)?;
        Ok(self.into_data())
    }

    pub fn as_verified(&self, uid: u64, author: u64) -> Result<(), (StatusCode, &'static str)> {
        self.as_active()?;
        self.as_authorized(uid, author)
    }

    pub fn verified_data(
        &self,
        uid: u64,
        author: u64,
    ) -> Result<&UserData, (StatusCode, &'static str)> {
        self.as_verified(uid, author)?;
        Ok(self.data())
    }

    pub fn verified_data_mut(
        &mut self,
        uid: u64,
        author: u64,
    ) -> Result<&mut UserData, (StatusCode, &'static str)> {
        self.as_verified(uid, author)?;
        Ok(self.data_mut())
    }

    pub fn into_verified_data(
        self,
        uid: u64,
        author: u64,
    ) -> Result<UserData, (StatusCode, &'static str)> {
        self.as_verified(uid, author)?;
        Ok(self.into_data())
    }
}

pub struct DbHelper<'a>(&'a sled::transaction::TransactionalTree);

impl<'a> DbHelper<'a> {
    pub fn new(tree: &'a sled::transaction::TransactionalTree) -> Self {
        Self(tree)
    }

    pub fn get<K: ToKey, V: DbType>(
        &self,
        key: &K,
    ) -> Result<Option<V>, (StatusCode, &'static str)> {
        let prefixed_key = [V::prefix().as_bytes(), &key.to_key()].concat();
        self.0
            .get(prefixed_key)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Fetch data failed"))?
            .map(|bytes| {
                rmp_serde::from_slice(&bytes)
                    .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Deserialize failed"))
            })
            .transpose()
    }

    pub fn get_or_not_found<K: ToKey, V: DbType>(
        &self,
        key: &K,
    ) -> Result<V, (StatusCode, &'static str)> {
        self.get(key)?.ok_or((StatusCode::NOT_FOUND, "Not found"))
    }

    pub fn insert<K: ToKey, V: DbType>(
        &self,
        key: &K,
        value: &V,
    ) -> Result<(), (StatusCode, &'static str)> {
        let prefixed_key = [V::prefix().as_bytes(), &key.to_key()].concat();
        let bytes = rmp_serde::to_vec(value)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Serialize failed"))?;
        self.0
            .insert(prefixed_key, bytes)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Insert data failed"))?;
        Ok(())
    }

    pub fn remove<K: ToKey, V: DbType>(&self, key: &K) -> Result<(), (StatusCode, &'static str)> {
        let prefixed_key = [V::prefix().as_bytes(), &key.to_key()].concat();
        self.0
            .remove(prefixed_key)
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Remove data failed"))?;
        Ok(())
    }
}

pub fn with_transaction<F, R>(db: &sled::Db, operation: F) -> Result<R, (StatusCode, &'static str)>
where
    F: Fn(DbHelper<'_>) -> Result<R, (StatusCode, &'static str)>,
{
    use sled::transaction::ConflictableTransactionError as CTError;
    db.transaction(|tx| operation(DbHelper::new(tx)).map_err(|e| CTError::Abort(e)))
        .map_err(|e| match e {
            sled::transaction::TransactionError::Abort(e) => e,
            _ => (StatusCode::CONFLICT, "Transaction conflict"),
        })
}

pub trait DbType: Serialize + DeserializeOwned {
    fn prefix() -> &'static str;
}

impl DbType for UserStatus {
    fn prefix() -> &'static str {
        "@"
    }
}

impl DbType for Topic {
    fn prefix() -> &'static str {
        "#"
    }
}

impl DbType for Top {
    fn prefix() -> &'static str {
        "!top"
    }
}

pub trait ToKey {
    fn to_key(&self) -> Vec<u8>;
}

impl ToKey for &'static str {
    fn to_key(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl ToKey for String {
    fn to_key(&self) -> Vec<u8> {
        self.as_bytes().to_vec()
    }
}

impl ToKey for u64 {
    fn to_key(&self) -> Vec<u8> {
        self.to_le_bytes().to_vec()
    }
}
