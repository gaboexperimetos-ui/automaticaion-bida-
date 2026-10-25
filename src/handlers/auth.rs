//! Authentication handlers: login form, login submit, logout, cambio de contraseña.

use argon2::{password_hash::PasswordHash, password_hash::PasswordVerifier, Argon2};
use axum::{
    body::Body,
    extract::{OriginalUri, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;

use crate::middleware::auth::{
    clear_session_cookie_for_request, generate_session_token, hash_session_token,
    make_session_cookie_for_transport, SESSION_COOKIE, SESSION_MAX_AGE,
};
use crate::models::{SetPasswordRequest, User};
use crate::AppState;

/// Login form data
#[derive(Deserialize)]
pub struct LoginForm {
    pub username: String,
    pub password: String,
}

/// Render the login page — professional design inspired by TailAdmin template.
pub async fn login_form() -> Response {
    let mut resp = Response::new(Body::from(LOGIN_PAGE_HTML));
    *resp.status_mut() = StatusCode::OK;
    resp.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

/// Variable de entorno con el token de invitación que habilita el registro.
///
/// Si **no está definida** (o está vacía) el registro público queda
/// deshabilitado: `POST /register` responde 403 y no crea ninguna cuenta.
/// El administrador comparte ese token con quien debe tener cuenta; el
/// cliente lo envía en `Authorization: Bearer <token>` o en el campo de
/// formulario `invite`.
pub const REGISTRATION_TOKEN_ENV: &str = "REGISTRATION_TOKEN";

/// Motivo honesto del rechazo cuando el registro está deshabilitado.
const REGISTER_DISABLED_MESSAGE: &str =
    "El registro está deshabilitado: pide una cuenta al administrador";

/// Motivo del rechazo cuando hay token configurado pero no coincide.
const REGISTER_INVALID_INVITE_MESSAGE: &str =
    "Invitación inválida: pide una cuenta al administrador";

/// Formulario de registro: datos del usuario + token de invitación.
///
/// `invite` es opcional: si no viene en el cuerpo, el token puede llegar en
/// la cabecera `Authorization: Bearer <token>`.
#[derive(Deserialize)]
pub struct RegisterForm {
    pub username: String,
    pub password: String,
    pub confirm: String,
    /// Token de invitación enviado por el administrador (opcional).
    #[serde(default)]
    pub invite: Option<String>,
}

/// Resultado de comprobar la invitación de registro.
#[derive(Debug, PartialEq, Eq)]
enum InviteCheck {
    /// Token válido: se puede crear la cuenta.
    Allowed,
    /// No hay token configurado: el registro está deshabilitado.
    Disabled,
    /// Hay token configurado, pero el recibido no coincide.
    Invalid,
}

/// Compara dos cadenas en tiempo constante (XOR manual, sin el crate `subtle`).
///
/// Recorre siempre `max(len_a, len_b)` bytes acumulando las diferencias con
/// OR: el tiempo depende solo de la longitud máxima, no de dónde ni de
/// cuántos bytes difieren. La comparación de longitudes va **al final**, sin
/// return temprano, para que el resultado no se filtren por un branch.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff: u8 = 0;
    let n = a.len().max(b.len());
    for i in 0..n {
        diff |= a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0);
    }
    diff == 0 && a.len() == b.len()
}

/// Extrae el token de una cabecera `Authorization: Bearer <token>`.
fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let raw = headers.get(http::header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, value) = raw.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("bearer") {
        return None;
    }
    let token = value.trim();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

/// Comprueba la invitación de registro contra el token configurado.
///
/// `expected = None` significa "no hay token configurado" → registro
/// deshabilitado (no es un fallo del cliente: es una decisión del
/// administrador).
fn check_registration_invite(
    expected: Option<&str>,
    headers: &HeaderMap,
    form_invite: Option<&str>,
) -> InviteCheck {
    let Some(expected) = expected else {
        return InviteCheck::Disabled;
    };
    // El token puede venir en la cabecera `Authorization: Bearer` o en el
    // campo de formulario `invite` (este último tiene prioridad si no hay
    // cabecera). Se copia a `String` para no mezclar dos préstamos de vida
    // distinta (cabecera vs. formulario) en la misma comparación.
    let provided: Option<String> = bearer_token(headers).map(str::to_string).or_else(|| {
        form_invite
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    });
    match provided {
        Some(token) if constant_time_eq(token.as_bytes(), expected.as_bytes()) => {
            InviteCheck::Allowed
        }
        _ => InviteCheck::Invalid,
    }
}

/// Puerta de entrada del registro: `Ok(())` si la invitación es válida,
/// `Err(resp)` con el 403 correspondiente si no lo es.
///
/// Es la única fuente de verdad para el rechazo, de modo que el mensaje y el
/// status nunca se desincronizan entre el camino del header y el del
/// formulario.
fn registration_gate(
    expected: Option<&str>,
    headers: &HeaderMap,
    form_invite: Option<&str>,
) -> Result<(), Response> {
    // Un token vacío (o solo espacios) se trata como "no configurado".
    let expected = expected.map(str::trim).filter(|t| !t.is_empty());
    match check_registration_invite(expected, headers, form_invite) {
        InviteCheck::Allowed => Ok(()),
        InviteCheck::Disabled => {
            tracing::warn!(
                "POST /register rechazado: {} no está definido; el registro público está deshabilitado",
                REGISTRATION_TOKEN_ENV
            );
            Err((StatusCode::FORBIDDEN, REGISTER_DISABLED_MESSAGE).into_response())
        }
        InviteCheck::Invalid => {
            tracing::warn!("POST /register rechazado: token de invitación ausente o incorrecto");
            Err((StatusCode::FORBIDDEN, REGISTER_INVALID_INVITE_MESSAGE).into_response())
        }
    }
}

/// Handle user registration.
///
/// **No es un endpoint público**: exige el token de invitación del
/// administrador (`REGISTRATION_TOKEN`). Si ese token no está configurado,
/// responde 403 y no crea ninguna cuenta; si está configurado, el cliente
/// debe enviarlo en `Authorization: Bearer <token>` o en el campo `invite`.
pub async fn register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<RegisterForm>,
) -> Response {
    // La invitación se comprueba **primero**: si no está autorizada no se
    // valida la contraseña, no se hashea nada y no se toca la base de datos.
    // Fuente única de verdad: `Config` lee `REGISTRATION_TOKEN` al arrancar y
    // descarta el valor vacío (un token vacío significaría "registro abierto").
    let expected = state.config.registration_token.clone();
    if let Err(resp) = registration_gate(expected.as_deref(), &headers, form.invite.as_deref()) {
        return resp;
    }

    if form.password != form.confirm {
        return (StatusCode::BAD_REQUEST, "Las contraseñas no coinciden").into_response();
    }
    if form.password.len() < 8 {
        return (StatusCode::BAD_REQUEST, "Mínimo 8 caracteres").into_response();
    }

    // Hash with Argon2
    use argon2::{
        password_hash::{PasswordHasher, PasswordVerifier, SaltString},
        Argon2,
    };
    let salt = SaltString::generate(&mut rand::thread_rng());
    let password_hash = match Argon2::default().hash_password(form.password.as_bytes(), &salt) {
        Ok(h) => h.to_string(),
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Error al crear cuenta: {}", e),
            )
                .into_response()
        }
    };

    let result = sqlx::query("INSERT INTO users (username, password_hash) VALUES (?, ?)")
        .bind(&form.username)
        .bind(&password_hash)
        .execute(&state.db)
        .await;

    match result {
        Ok(_) => (StatusCode::OK, "Cuenta creada. Ahora inicia sesión.").into_response(),
        Err(e) => {
            if e.to_string().contains("UNIQUE") {
                (StatusCode::CONFLICT, "Ese usuario ya existe").into_response()
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Error en la base de datos",
                )
                    .into_response()
            }
        }
    }
}

/// Handle login form submission.
pub async fn login_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    OriginalUri(uri): OriginalUri,
    form: Form<LoginForm>,
) -> Response {
    // Validate Origin/Referer when present (CSRF defense)
    if let Some(rejection) = check_login_origin(&headers) {
        return rejection;
    }

    // Look up user
    let user = sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, created_at FROM users WHERE username = ?",
    )
    .bind(&form.username)
    .fetch_optional(&state.db)
    .await;

    match user {
        Ok(Some(user)) => {
            // Verify password with Argon2
            use argon2::{password_hash::PasswordHash, PasswordVerifier};
            let parsed_hash = match PasswordHash::new(&user.password_hash) {
                Ok(h) => h,
                Err(_) => {
                    // Hash corrupto en la fila: mismo mensaje genérico que un
                    // fallo normal (no filtra nada extra).
                    return login_error_response(&headers);
                }
            };

            if Argon2::default()
                .verify_password(form.password.as_bytes(), &parsed_hash)
                .is_ok()
            {
                // Create session with opaque CSPRNG token
                let raw_token = generate_session_token();
                let row_id = hash_session_token(&raw_token, &state.config.session_secret);
                let now = chrono::Utc::now();
                let expires = now + chrono::Duration::seconds(SESSION_MAX_AGE as i64);

                let _ = sqlx::query(
                    r#"INSERT INTO sessions (id, user_id, expires_at) VALUES (?, ?, ?)"#,
                )
                .bind(&row_id)
                .bind(user.id)
                .bind(expires.to_rfc3339())
                .execute(&state.db)
                .await;

                // Redirect to dashboard with session cookie
                login_success_response(&make_session_cookie_for_transport(
                    &raw_token, &headers, &uri,
                ))
            } else {
                // Dummy work to prevent timing attacks
                use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
                let _ = Argon2::default()
                    .hash_password(b"dummy", &SaltString::generate(&mut rand::thread_rng()))
                    .ok();
                login_error_response(&headers)
            }
        }
        Ok(None) => {
            // Same dummy work even for missing users
            use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
            let _ = Argon2::default()
                .hash_password(b"dummy", &SaltString::generate(&mut rand::thread_rng()))
                .ok();
            // Mismo cuerpo exacto que el caso de contraseña incorrecta: la
            // respuesta NO revela si el usuario existe (anti-enumeración).
            login_error_response(&headers)
        }
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "Error en el servidor").into_response(),
    }
}

/// Respuesta 302 del login exitoso: `Location` para los clientes nativos y
/// `HX-Redirect` para los que hablan htmx.
///
/// htmx **no** navega por sí solo cuando la respuesta es un 3xx: el XHR
/// sigue el redirect, y el HTML de `/dashboard` se inyecta dentro de
/// `#login-container` dejando la URL en `/login` (el usuario ve el dashboard
/// "dentro" de la tarjeta de login, sin salir nunca de `/login`).
///
/// Por eso además del 302 se manda `HX-Redirect: /dashboard`: es la cabecera
/// que htmx entiende para hacer una navegación real. El status se mantiene
/// en 302 (los clientes nativos y la suite de tests lo esperan) y la
/// cookie de sesión se sigue mandando en la misma respuesta.
fn login_success_response(set_cookie: &str) -> Response {
    http::Response::builder()
        .status(302)
        .header("Location", "/dashboard")
        .header("HX-Redirect", HeaderValue::from_static("/dashboard"))
        .header("Set-Cookie", set_cookie)
        .body(Body::empty())
        .expect("redirect response builder")
}

/// Handle logout — deletes session row from DB.
pub async fn logout(State(state): State<Arc<AppState>>, req: Request<Body>) -> Response {
    let raw = match crate::middleware::auth::extract_session_cookie(&req) {
        Some(v) => v,
        None => return axum::response::Redirect::to("/login").into_response(),
    };

    let hashed = hash_session_token(&raw, &state.config.session_secret);
    // Both the hashed id (rows minted by this code) and the raw value
    // (legacy pre-Mission-11 rows) are deleted, so the session is really
    // revoked and not just ignored by the browser.
    let _ = sqlx::query("DELETE FROM sessions WHERE id = ? OR id = ?")
        .bind(&hashed)
        .bind(&raw)
        .execute(&state.db)
        .await;

    let mut response = Response::new(Body::from(LOGIN_PAGE_HTML));
    *response.status_mut() = StatusCode::OK;
    response.headers_mut().insert(
        http::header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    response.headers_mut().insert(
        http::header::SET_COOKIE,
        // The deletion cookie must carry the same attributes as the cookie it
        // deletes, or the browser may keep the old one.
        HeaderValue::from_str(&clear_session_cookie_for_request(&req)).unwrap(),
    );
    response
}

/// Handle set-password (admin setup / change).
///
/// Endpoint de administración **sin ruta registrada**: fija la contraseña del
/// usuario `admin`. El autoservicio (usuario → su propia contraseña) es
/// [`password_submit`], que además exige la contraseña actual y cierra el
/// resto de sesiones.
///
/// Se conserva el comportamiento histórico (estados y mensajes) para no romper
/// a quien lo use por código; el hash y el `UPDATE` se comparten ahora con
/// [`store_password`], de modo que no hay dos caminos que escriban la tabla.
pub async fn set_password(
    State(state): State<Arc<AppState>>,
    Form(form): Form<SetPasswordRequest>,
) -> Response {
    match form.validate() {
        Ok(_) => {}
        Err(e) => {
            return (StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    }

    match store_password(&state, UserKey::Username("admin"), &form.password).await {
        Ok(affected) if affected > 0 => (StatusCode::OK, "Contraseña actualizada").into_response(),
        Ok(_) => (StatusCode::NOT_FOUND, "No se encontró al usuario 'admin'").into_response(),
        Err(PasswordStoreError::Hash(e)) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Database error: {}", e),
        )
            .into_response(),
        Err(PasswordStoreError::Db(e)) => {
            (StatusCode::INTERNAL_SERVER_ERROR, format!("{}", e)).into_response()
        }
    }
}

// ---------------------------------------------------------------------------
// Cambio de contraseña (autoservicio del usuario)
// ---------------------------------------------------------------------------
//
// Sin tokens CSRF: el proyecto no tiene ninguno (la defensa es
// `SameSite=Strict` + validación de `Origin` en `POST /login`, ver
// `middleware::auth`). Por eso este flujo **exige la contraseña actual**: un
// POST forzado desde otro sitio no puede completarlo sin conocerla, y el
// cambio además invalida el resto de sesiones del usuario.

/// Ruta (y `action` del formulario) del cambio de contraseña.
///
/// Registro sugerido en `handlers::mod::router`, dentro del grupo protegido
/// por `require_auth_state`:
///
/// ```text
/// .route("/settings/password", get(auth::password_page).post(auth::password_submit))
/// ```
pub const PASSWORD_ROUTE: &str = "/settings/password";

/// Longitud mínima (en caracteres) exigida a la contraseña nueva.
pub const MIN_NEW_PASSWORD_LEN: usize = 12;

/// Formulario de cambio de contraseña.
///
/// `username` va oculto y se ignora cuando hay sesión (el usuario real es el
/// de la sesión). Sólo es operativo con `AUTH_DISABLED=true`, donde no existe
/// sesión que identifique a nadie y el campo se muestra editable.
#[derive(Deserialize)]
pub struct ChangePasswordForm {
    #[serde(default)]
    pub username: String,
    pub current_password: String,
    pub new_password: String,
    pub confirm_password: String,
}

/// Usuario y sesión resueltos desde la cookie de sesión.
struct SessionUser {
    user_id: i64,
    username: String,
    password_hash: String,
    /// Id de la fila de `sessions` (el token hasheado) — la sesión que se
    /// conserva al invalidar las demás.
    session_id: String,
}

/// Mensaje mostrado en la página de cambio de contraseña.
enum Feedback {
    Error(String),
    Success(String),
}

/// GET — página con el formulario de cambio de contraseña.
///
/// Estilo Stitch del login: se reutiliza literalmente su bloque `<head>`
/// (tokens de Tailwind, Material Symbols, htmx) y sus clases de tarjeta y de
/// campo, de modo que ambas páginas se ven iguales.
///
/// Sin sesión válida: si la autenticación está activa se redirige a
/// `/login`; si está deshabilitada (`AUTH_DISABLED=true`) se muestra el
/// formulario con el campo de usuario editable, porque no hay sesión que
/// diga de quién es la contraseña.
pub async fn password_page(State(state): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let session = session_user(&state, &headers).await;

    match session {
        Some(user) => html_response(
            StatusCode::OK,
            password_page_html(&user.username, false, None),
        ),
        None if state.config.auth_disabled => {
            html_response(StatusCode::OK, password_page_html("", true, None))
        }
        None => axum::response::Redirect::to("/login").into_response(),
    }
}

/// POST — aplica el cambio de contraseña.
///
/// Pasos: (1) se identifica al usuario (sesión actual; con
/// `AUTH_DISABLED=true`, el que indica el formulario), (2) se verifica la
/// contraseña actual con Argon2, (3) se valida la nueva (mínimo 12
/// caracteres, confirmación coincidente y distinta de la actual), (4) se
/// re-hashea con Argon2 y se actualiza la fila de `users`, (5) se invalidan
/// las demás sesiones del usuario y (6) se responde 200 con el mensaje de
/// éxito o 400 con el error, siempre en español.
///
/// La respuesta **nunca** distingue "el usuario no existe" de "la contraseña
/// actual es incorrecta": ambos casos devuelven el mismo mensaje.
pub async fn password_submit(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Form(form): Form<ChangePasswordForm>,
) -> Response {
    // (1) Usuario objetivo.
    let session = session_user(&state, &headers).await;
    let (user_id, username, stored_hash, editable) = match &session {
        Some(user) => (
            Some(user.user_id),
            user.username.clone(),
            Some(user.password_hash.clone()),
            false,
        ),
        None if state.config.auth_disabled => {
            let typed = form.username.trim().to_string();
            match find_user_by_username(&state, &typed).await {
                Some(user) => (
                    Some(user.id),
                    user.username.clone(),
                    Some(user.password_hash),
                    true,
                ),
                None => (None, typed, None, true),
            }
        }
        None => {
            return Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header(http::header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(Body::from("Sesión no válida: inicia sesión de nuevo."))
                .expect("respuesta 401");
        }
    };

    // (2) Contraseña actual. Mismo veredicto para usuario inexistente y para
    // contraseña incorrecta: la respuesta no enumera cuentas.
    let current_ok = match stored_hash.as_deref() {
        Some(hash) => match PasswordHash::new(hash) {
            Ok(parsed) => Argon2::default()
                .verify_password(form.current_password.as_bytes(), &parsed)
                .is_ok(),
            Err(_) => false,
        },
        None => false,
    };
    if !current_ok {
        // Trabajo simétrico para que el tiempo de respuesta no revele si la
        // cuenta existe (igual que hace el login).
        let _ = hash_password("dummy");
        return password_error_response(&username, editable, "La contraseña actual no es correcta");
    }

    // (3) Reglas de la contraseña nueva.
    if let Err(msg) = validate_new_password(
        &form.new_password,
        &form.confirm_password,
        &form.current_password,
    ) {
        return password_error_response(&username, editable, &msg);
    }

    let user_id = match user_id {
        Some(id) => id,
        None => {
            return password_error_response(
                &username,
                editable,
                "No se pudo actualizar la contraseña",
            )
        }
    };

    // (4) Re-hash y UPDATE de la fila de `users`.
    match store_password(&state, UserKey::Id(user_id), &form.new_password).await {
        Ok(affected) if affected > 0 => {}
        Ok(_) => {
            return password_error_response(
                &username,
                editable,
                "No se pudo actualizar la contraseña",
            )
        }
        Err(PasswordStoreError::Hash(_)) => {
            return password_error_response(
                &username,
                editable,
                "No se pudo guardar la contraseña nueva",
            )
        }
        Err(PasswordStoreError::Db(_)) => {
            return password_error_response(
                &username,
                editable,
                "Error al guardar la contraseña nueva",
            )
        }
    }

    // (5) Invalidar el resto de sesiones del usuario. En `sessions` no hay
    // columna `token`: la fila se identifica por `id`, que guarda el hash
    // SHA-256 del token (las filas heredadas guardan el token en crudo), así
    // que "session actual" es exactamente el `id` de la fila resuelta.
    let revoked = revoke_other_sessions(
        &state,
        user_id,
        session.as_ref().map(|s| s.session_id.as_str()),
    )
    .await;

    // (6) Éxito.
    let message = if revoked > 0 {
        format!(
            "Contraseña actualizada. Se cerraron {} sesión{} abierta{}.",
            revoked,
            if revoked == 1 { "" } else { "es" },
            if revoked == 1 { "" } else { "s" },
        )
    } else {
        "Contraseña actualizada. No había otras sesiones abiertas.".to_string()
    };

    html_response(
        StatusCode::OK,
        password_page_html(&username, editable, Some(&Feedback::Success(message))),
    )
}

/// Valida la contraseña nueva. Devuelve el mensaje de error en español.
fn validate_new_password(
    new_password: &str,
    confirm_password: &str,
    current_password: &str,
) -> Result<(), String> {
    if new_password.chars().count() < MIN_NEW_PASSWORD_LEN {
        return Err(format!(
            "La nueva contraseña debe tener al menos {} caracteres",
            MIN_NEW_PASSWORD_LEN
        ));
    }
    if new_password != confirm_password {
        return Err("Las contraseñas nuevas no coinciden".to_string());
    }
    if new_password == current_password {
        return Err("La nueva contraseña debe ser distinta de la actual".to_string());
    }
    Ok(())
}

/// Resuelve el usuario de la cookie de sesión actual.
///
/// Búsqueda por el hash del token primero y por el token en crudo después
/// (filas heredadas de antes del cambio a tokens opacos), igual que hace
/// `require_auth_state`. Las sesiones expiradas no cuentan.
async fn session_user(state: &AppState, headers: &HeaderMap) -> Option<SessionUser> {
    let raw = session_cookie_from_headers(headers)?;
    let hashed = hash_session_token(&raw, &state.config.session_secret);

    let row = sqlx::query_as::<_, (String, i64, String, String, String)>(
        "SELECT s.id, s.user_id, u.username, u.password_hash, s.expires_at \
         FROM sessions s JOIN users u ON u.id = s.user_id \
         WHERE s.id = ? OR s.id = ?",
    )
    .bind(&hashed)
    .bind(&raw)
    .fetch_optional(&state.db)
    .await
    .ok()??;

    let (session_id, user_id, username, password_hash, expires_at) = row;
    if expires_at <= chrono::Utc::now().to_rfc3339() {
        return None;
    }

    Some(SessionUser {
        user_id,
        username,
        password_hash,
        session_id,
    })
}

/// Busca un usuario por nombre (sólo para el modo sin autenticación).
async fn find_user_by_username(state: &AppState, username: &str) -> Option<User> {
    if username.is_empty() {
        return None;
    }
    sqlx::query_as::<_, User>(
        "SELECT id, username, password_hash, created_at FROM users WHERE username = ?",
    )
    .bind(username)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
}

/// Cierra las sesiones del usuario salvo la actual.
///
/// La tabla `sessions` no tiene columna `token`: la fila se identifica por
/// `id`, que guarda `SHA256(SESSION_SECRET:token)` (o el token en crudo en las
/// filas heredadas). Por eso "la sesión actual" es el `id` de la fila
/// resuelta, y lo que se borra son las filas *distintas* de ésa.
/// Devuelve cuántas filas se cerraron.
async fn revoke_other_sessions(
    state: &AppState,
    user_id: i64,
    current_session_id: Option<&str>,
) -> u64 {
    let result = match current_session_id {
        Some(id) => {
            sqlx::query("DELETE FROM sessions WHERE user_id = ? AND id <> ?")
                .bind(user_id)
                .bind(id)
                .execute(&state.db)
                .await
        }
        None => {
            sqlx::query("DELETE FROM sessions WHERE user_id = ?")
                .bind(user_id)
                .execute(&state.db)
                .await
        }
    };
    result.map(|res| res.rows_affected()).unwrap_or(0)
}

/// Cómo se localiza al usuario en el `UPDATE` de `users`.
enum UserKey<'a> {
    Id(i64),
    Username(&'a str),
}

/// Fallo al guardar la contraseña.
enum PasswordStoreError {
    /// No se pudo generar el hash (fallo de Argon2).
    Hash(String),
    /// Fallo de base de datos.
    Db(String),
}

/// Hashea una contraseña con Argon2 (formato PHC) y la devuelve como texto.
fn hash_password(password: &str) -> Result<String, String> {
    use argon2::{password_hash::SaltString, Argon2, PasswordHasher};
    let salt = SaltString::generate(&mut rand::thread_rng());
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

/// Escribe la contraseña (ya hasheada por [`store_password`]) en `users`.
///
/// Único punto de escritura de `users.password_hash`: lo comparten
/// [`set_password`] (por nombre de usuario) y [`password_submit`] (por id).
/// Devuelve el número de filas afectadas.
async fn store_password(
    state: &AppState,
    key: UserKey<'_>,
    password: &str,
) -> Result<u64, PasswordStoreError> {
    let hash = hash_password(password).map_err(PasswordStoreError::Hash)?;

    let result = match key {
        UserKey::Id(id) => {
            sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
                .bind(&hash)
                .bind(id)
                .execute(&state.db)
                .await
        }
        UserKey::Username(name) => {
            sqlx::query("UPDATE users SET password_hash = ? WHERE username = ?")
                .bind(&hash)
                .bind(name)
                .execute(&state.db)
                .await
        }
    };

    result
        .map(|res| res.rows_affected())
        .map_err(|e| PasswordStoreError::Db(e.to_string()))
}

/// Extrae el valor crudo de la cookie de sesión de las cabeceras.
///
/// Mismo formato que `middleware::auth::extract_session_cookie` (que recibe la
/// `Request` completa); aquí sólo se dispone de las cabeceras porque el
/// formulario se procesa con `Form`, que consume el cuerpo de la petición.
fn session_cookie_from_headers(headers: &HeaderMap) -> Option<String> {
    let cookie = headers.get(http::header::COOKIE)?.to_str().ok()?;
    let prefix = format!("{}=", SESSION_COOKIE);
    cookie.split(';').find_map(|part| {
        let part = part.trim();
        part.strip_prefix(prefix.as_str())
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Escapa texto para insertarlo en HTML.
fn html_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Respuesta HTML con el estado indicado.
fn html_response(status: StatusCode, body: String) -> Response {
    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .expect("respuesta html")
}

/// Respuesta 400 con la página de contraseña y el error en español.
fn password_error_response(username: &str, editable: bool, message: &str) -> Response {
    html_response(
        StatusCode::BAD_REQUEST,
        password_page_html(
            username,
            editable,
            Some(&Feedback::Error(message.to_string())),
        ),
    )
}

/// Validate Origin/Referer on POST /login (CSRF defense).
fn check_login_origin(headers: &HeaderMap) -> Option<Response> {
    let origin = headers.get(http::header::ORIGIN)?;
    let host = headers.get(http::header::HOST)?;

    let origin_str = origin.to_str().ok()?;
    let host_str = host.to_str().ok()?;

    if origin_str.contains(host_str) || host_str.contains(origin_str) {
        None
    } else {
        Some((StatusCode::FORBIDDEN, "Origen no autorizado").into_response())
    }
}

/// Fragment HTML devuelto cuando falla un inicio de sesión.
///
/// Honesto sobre *qué* falló (las credenciales) y mudo sobre *por qué*: las
/// ramas "usuario no encontrado" y "contraseña incorrecta" devuelven este
/// mismo cuerpo byte a byte, así que la respuesta nunca enumera cuentas.
/// El título genérico ("Credenciales incorrectas") se apoya en el texto
/// histórico ("Usuario o contraseña incorrectos"), que la suite de tests
/// comprueba literalmente — por eso se pega tal cual bajo el título.
///
/// No se separa el caso "usuario no encontrado": el rate limiter ya
/// responde por su cuenta con un 429 genérico ("Too Many Requests"), así que
/// no existe un rate-limit similar que justifique un mensaje distinto (solo
/// filtraría la existencia de la cuenta).
const LOGIN_ERROR_HTML: &str = r##"<div class="flex items-start gap-3 rounded-lg border border-[#ff8a80]/35 bg-[#ff8a80]/10 px-3 py-2.5" role="alert">
  <span class="material-symbols-outlined text-[20px] text-[#ff8a80]">error</span>
  <div>
    <p class="text-sm font-semibold text-[#ffb4ab]">Credenciales incorrectas</p>
    <p class="mt-0.5 text-xs leading-relaxed text-on-surface-variant">Usuario o contraseña incorrectos. Revisa el usuario y la contraseña e inténtalo de nuevo.</p>
  </div>
</div>"##;

/// Respuesta 401 (HTML) para un intento de login fallido.
///
/// Mismo cuerpo para todos los fallos de credenciales: no filtra si el
/// usuario existe.
///
/// `headers` distingue una petición htmx de una nativa: htmx no hace swap con
/// respuestas 4xx, así que en ese caso se devuelve solo el fragmento y el
/// script de la página lo inyecta en el slot `#login-error`. Una petición
/// nativa (p. ej. sin htmx porque no carga el CDN) recibiría el fragmento
/// suelto como página entera, así que en ese caso se devuelve la página de
/// login completa con el error ya puesto.
fn login_error_response(headers: &HeaderMap) -> Response {
    let body = if headers.get("HX-Request").is_some() {
        LOGIN_ERROR_HTML.to_string()
    } else {
        LOGIN_PAGE_HTML.replace(
            "<div id=\"login-error\" class=\"mt-5\"></div>",
            &format!(
                "<div id=\"login-error\" class=\"mt-5\">{}</div>",
                LOGIN_ERROR_HTML
            ),
        )
    };
    Response::builder()
        .status(StatusCode::UNAUTHORIZED)
        .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
        .body(Body::from(body))
        .expect("login error response builder")
}

const LOGIN_PAGE_HTML: &str = r##"<!DOCTYPE html>
<html lang="es">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>NOVA — Iniciar sesión</title>
    <link rel="preconnect" href="https://fonts.googleapis.com">
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
    <link href="https://fonts.googleapis.com/css2?family=Material+Symbols+Outlined&family=DM+Sans:wght@400;500;600;700&family=Space+Grotesk:wght@500;600;700&display=swap" rel="stylesheet">
    <script src="https://unpkg.com/htmx.org@1.9.12"></script>
    <script src="https://cdn.tailwindcss.com"></script>
    <script>
        // Stitch — "Synthetic Cognition" design tokens
        tailwind.config = {
            theme: {
                extend: {
                    colors: {
                        background: '#090b11',
                        'surface-container-lowest': '#090c12',
                        'surface-container-low': '#0d1017',
                        'surface-container': '#11151d',
                        'surface-container-high': '#161b25',
                        'surface-container-highest': '#1c2230',
                        'on-surface': '#f2f4f8',
                        'on-surface-variant': '#a2abbc',
                        outline: '#747f93',
                        'outline-variant': '#2a3242',
                        primary: '#dce1ff',
                        'on-primary': '#121728',
                        'primary-container': '#a9b6ff',
                        'on-primary-container': '#121728',
                        secondary: '#76d7c0',
                        'on-secondary': '#121728',
                        'secondary-container': '#1f4a40',
                        'on-secondary-container': '#d4f8ed',
                        'surface-tint': '#a9b6ff',
                        'text-primary': '#f2f4f8',
                        'text-secondary': '#a2abbc',
                        'text-muted': '#747f93',
                        canvas: '#090c12',
                        'input-bg': '#0c1017',
                    },
                    fontFamily: {
                        manrope: ['"Space Grotesk"', 'sans-serif'],
                        hanken: ['"DM Sans"', 'sans-serif'],
                    },
                    boxShadow: {
                        glow: '0 0 12px rgba(169, 182, 255, 0.22)',
                        'glow-hover': '0 0 20px rgba(169, 182, 255, 0.35)',
                    },
                },
            },
        };
    </script>
    <link rel="stylesheet" href="/polish.css">
</head>
<body class="auth-body">
    <main class="auth-shell">
        <section class="auth-intro" aria-label="NOVA Academic OS">
            <div class="auth-brand">
                <span class="auth-brand-mark" aria-hidden="true">N</span>
                <div><strong>NOVA</strong><small>ACADEMIC OS</small></div>
                <span class="auth-version">3.0</span>
            </div>
            <div class="auth-intro-copy">
                <p class="auth-kicker">Tu espacio académico</p>
                <h1>Todo lo que estudias.<br><em>Por fin, en su lugar.</em></h1>
                <p>Notas, materias, IA y documentos conectados en un entorno privado que trabaja a tu ritmo.</p>
            </div>
            <div class="auth-features" aria-label="Incluye">
                <div><span class="material-symbols-outlined">auto_awesome</span><div><strong>IA con contexto</strong><p>Tus apuntes y materias, disponibles al preguntar.</p></div></div>
                <div><span class="material-symbols-outlined">account_tree</span><div><strong>Todo conectado</strong><p>Agenda, conocimiento y archivos sin saltar entre apps.</p></div></div>
                <div><span class="material-symbols-outlined">shield_locked</span><div><strong>Privado por diseño</strong><p>Tu espacio vive en tu red local.</p></div></div>
            </div>
            <footer class="auth-intro-foot"><span><i></i> Sistema local disponible</span><span>NOVA · 3.0</span></footer>
        </section>
        <section class="auth-access" aria-label="Iniciar sesión">
            <div class="auth-card" id="login-container">
                <header class="auth-card-head">
                    <span class="auth-card-icon material-symbols-outlined" aria-hidden="true">lock_open</span>
                    <p>ACCESO PRIVADO</p>
                    <h2>Bienvenido de nuevo.</h2>
                    <span>Entra para continuar donde lo dejaste.</span>
                </header>
                <!-- Slot de error: htmx no hace swap con respuestas 4xx, así que el
                     fragmento LOGIN_ERROR_HTML (401) se inyecta aquí por JS. -->
                <div id="login-error" class="mt-5"></div>
                <!-- `method`/`action` son el respaldo SIN JavaScript: sin htmx el
                     navegador haría GET a /login con la contraseña en la URL (y
                     nunca habría POST ni error visible). Con htmx manda `hx-post`
                     (el verbo y la ruta los toma htmx, no los atributos nativos). -->
                <form method="post" action="/login" hx-post="/login" hx-swap="outerHTML" hx-target="#login-container" class="auth-form">
                    <div class="auth-field">
                        <label for="username">Usuario</label>
                        <div class="auth-input-wrap"><span class="material-symbols-outlined" aria-hidden="true">person</span><input id="username" type="text" name="username" required autocomplete="username" autofocus placeholder="Escribe tu usuario"></div>
                    </div>
                    <div class="auth-field">
                        <label for="password">Contraseña</label>
                        <div class="auth-input-wrap auth-password">
                            <span class="material-symbols-outlined" aria-hidden="true">key</span>
                            <input id="password" type="password" name="password" required autocomplete="current-password" placeholder="Escribe tu contraseña">
                            <button type="button" id="toggle-password" aria-label="Mostrar contraseña" aria-pressed="false"><span class="material-symbols-outlined" id="toggle-password-icon">visibility</span></button>
                        </div>
                    </div>
                    <div class="auth-options">
                        <label class="auth-remember"><input type="checkbox" class="sr-only peer"><span aria-hidden="true">✓</span> Mantener sesión</label>
                        <a href="#">¿Olvidaste tu contraseña?</a>
                    </div>
                    <button type="submit" class="auth-submit"><span>Iniciar sesión</span><span class="material-symbols-outlined" aria-hidden="true">arrow_forward</span></button>
                </form>
                <p class="auth-local"><span class="material-symbols-outlined">lock</span> Acceso solo desde la red local</p>
                <p class="auth-register">¿No tienes cuenta? <a href="/register">Regístrate aquí</a></p>
            </div>
            <p class="auth-access-meta">NOVA 3.0 <span>·</span> Tu conocimiento, bajo tu control.</p>
        </section>
    </main>
    <script>
        // UX mínima de la tarjeta de login:
        //  1) badge mostrar/ocultar contraseña
        //  2) mostrar el error de login (htmx no hace swap con respuestas 4xx,
        //     así que el 401 se inyecta a mano en el slot #login-error)
        (function () {
            var input = document.getElementById('password');
            var btn = document.getElementById('toggle-password');
            var icon = document.getElementById('toggle-password-icon');
            var slot = document.getElementById('login-error');

            if (input && btn && icon) {
                btn.addEventListener('click', function () {
                    var show = input.type === 'password';
                    input.type = show ? 'text' : 'password';
                    icon.textContent = show ? 'visibility_off' : 'visibility';
                    btn.setAttribute('aria-pressed', show ? 'true' : 'false');
                    btn.setAttribute('aria-label', show ? 'Ocultar contraseña' : 'Mostrar contraseña');
                });
            }

            if (slot) {
                var box = function (title, hint) {
                    return '<div class="flex items-start gap-3 rounded-lg border border-[#ff8a80]/35 bg-[#ff8a80]/10 px-3 py-2.5" role="alert">' +
                        '<span class="material-symbols-outlined text-[20px] text-[#ff8a80]">error</span>' +
                        '<div><p class="text-sm font-semibold text-[#ffb4ab]">' + title + '</p>' +
                        '<p class="mt-0.5 text-xs leading-relaxed text-on-surface-variant">' + hint + '</p></div></div>';
                };
                document.body.addEventListener('htmx:responseError', function (evt) {
                    var xhr = evt.detail && evt.detail.xhr;
                    if (!xhr) return;
                    if (xhr.status === 401) {
                        slot.innerHTML = xhr.responseText || '';
                    } else if (xhr.status === 429) {
                        slot.innerHTML = box('Demasiados intentos', 'Espera un minuto antes de volver a intentarlo.');
                    } else {
                        slot.innerHTML = box('Error del servidor', 'No pudimos iniciar la sesión. Inténtalo de nuevo en un momento.');
                    }
                });
            }

            // Redirección real tras un login exitoso.
            //
            // htmx no navega con un 302: el XHR sigue el redirect y el HTML de
            // /dashboard se inyecta dentro de #login-container, dejando la URL
            // en /login. Así que:
            //  - si la respuesta trae HX-Redirect, navegamos a ese valor;
            //  - si el XHR terminó en /dashboard (porque siguió el 302) se
            //    navega igual: los headers del 302 no son visibles desde JS
            //    (el navegador los descarta al seguir el redirect), pero sí
            //    la URL final de la respuesta.
            (function () {
                var navigating = false;

                function redirectTarget(xhr) {
                    if (!xhr || xhr.status < 200 || xhr.status >= 300) return null;

                    var header = xhr.getResponseHeader ? xhr.getResponseHeader('HX-Redirect') : null;
                    if (header) return header;

                    var url = xhr.responseURL;
                    if (!url) return null;
                    try {
                        if (new URL(url, window.location.href).pathname === '/dashboard') {
                            return '/dashboard';
                        }
                    } catch (e) {
                        // URL no parseable: sin redirección.
                    }
                    return null;
                }

                // Evita que el HTML de /dashboard se inyecte en #login-container.
                document.body.addEventListener('htmx:beforeSwap', function (evt) {
                    var xhr = evt.detail && evt.detail.xhr;
                    if (redirectTarget(xhr)) {
                        evt.detail.shouldSwap = false;
                    }
                });

                document.body.addEventListener('htmx:afterRequest', function (evt) {
                    var xhr = evt.detail && evt.detail.xhr;
                    var target = redirectTarget(xhr);
                    if (target && !navigating) {
                        navigating = true;
                        window.location.href = target;
                    }
                });
            })();
        })();
    </script>
</body>
</html>"##;

/// Título de la página de login (se sustituye en la de contraseña).
const LOGIN_TITLE: &str = "<title>NOVA — Iniciar sesión</title>";

/// Título de la página de cambio de contraseña.
const PASSWORD_TITLE: &str = "<title>NOVA — Cambiar contraseña</title>";

/// Bloque `<head>…</head>` de la página de login, reutilizado tal cual en la
/// página de contraseña.
///
/// Se extrae del literal en vez de duplicarlo: así la página de contraseña usa
/// exactamente los mismos tokens de Stitch, la misma configuración de Tailwind
/// y el mismo htmx que el login, y no hay dos listas de clases que se
/// desincronicen. La página de login no se toca (ni un byte), de modo que los
/// cuerpos 401 que la incluyen siguen siendo idénticos.
fn stitch_head_html() -> &'static str {
    let start = LOGIN_PAGE_HTML
        .find("<head>")
        .expect("la página de login tiene <head>");
    let end = LOGIN_PAGE_HTML
        .find("</head>")
        .expect("la página de login tiene </head>")
        + "</head>".len();
    &LOGIN_PAGE_HTML[start..end]
}

/// Script inline de la página de contraseña: mostrar/ocultar cada campo.
const PASSWORD_PAGE_JS: &str = r#"(function () {
    ['current_password', 'new_password', 'confirm_password'].forEach(function (id) {
        var input = document.getElementById(id);
        var btn = document.getElementById('toggle-' + id);
        if (!input || !btn) return;
        btn.addEventListener('click', function () {
            var show = input.type === 'password';
            input.type = show ? 'text' : 'password';
            var icon = btn.querySelector('span');
            if (icon) icon.textContent = show ? 'visibility_off' : 'visibility';
            btn.setAttribute('aria-pressed', show ? 'true' : 'false');
            btn.setAttribute('aria-label', show ? 'Ocultar contraseña' : 'Mostrar contraseña');
        });
    });
})();"#;

/// Banner de feedback (error o éxito) de la página de contraseña.
fn feedback_html(feedback: &Feedback) -> String {
    match feedback {
        Feedback::Error(message) => format!(
            r##"<div class="flex items-start gap-3 rounded-lg border border-[#ff8a80]/35 bg-[#ff8a80]/10 px-3 py-2.5" role="alert">
  <span class="material-symbols-outlined text-[20px] text-[#ff8a80]">error</span>
  <div>
    <p class="text-sm font-semibold text-[#ffb4ab]">No se pudo cambiar la contraseña</p>
    <p class="mt-0.5 text-xs leading-relaxed text-on-surface-variant">{}</p>
  </div>
</div>"##,
            html_escape(message)
        ),
        Feedback::Success(message) => format!(
            r##"<div class="flex items-start gap-3 rounded-lg border border-secondary/35 bg-secondary/10 px-3 py-2.5" role="status">
  <span class="material-symbols-outlined text-[20px] text-secondary">check_circle</span>
  <div>
    <p class="text-sm font-semibold text-secondary">Contraseña actualizada</p>
    <p class="mt-0.5 text-xs leading-relaxed text-on-surface-variant">{}</p>
  </div>
</div>"##,
            html_escape(message)
        ),
    }
}

/// Página de cambio de contraseña.
///
/// `username_editable` = `true` sólo cuando no hay sesión (modo
/// `AUTH_DISABLED=true`): entonces el campo de usuario se puede escribir y es
/// el que se usa. Con sesión, el campo va de sólo lectura y el servidor usa
/// el usuario de la sesión, ignorando lo que llegue en el formulario.
fn password_page_html(
    username: &str,
    username_editable: bool,
    feedback: Option<&Feedback>,
) -> String {
    let banner = feedback.map(feedback_html).unwrap_or_default();

    let username_input = if username_editable {
        format!(
            r##"<input id="username" type="text" name="username" value="{}" required autocomplete="username" placeholder="usuario" class="w-full h-10 px-3 bg-surface-container border border-[#30394b] rounded text-on-surface placeholder:text-muted transition-colors focus:outline-none focus:ring-2 focus:ring-surface-tint focus:border-surface-tint focus:ring-offset-2 focus:ring-offset-background">"##,
            html_escape(username)
        )
    } else {
        format!(
            r##"<input id="username" type="text" name="username" value="{}" readonly autocomplete="username" class="w-full h-10 px-3 bg-surface-container/60 border border-[#30394b] rounded text-on-surface-variant transition-colors">"##,
            html_escape(username)
        )
    };

    let head = stitch_head_html().replace(LOGIN_TITLE, PASSWORD_TITLE);

    format!(
        r##"<!DOCTYPE html>
<html lang="es">
{head}
<body class="auth-body">
    <main class="auth-shell">
        <section class="auth-intro" aria-label="NOVA Academic OS">
            <div class="auth-brand">
                <span class="auth-brand-mark" aria-hidden="true">N</span>
                <div><strong>NOVA</strong><small>ACADEMIC OS</small></div>
                <span class="auth-version">3.0</span>
            </div>
            <div class="auth-intro-copy">
                <p class="auth-kicker">Seguridad de tu cuenta</p>
                <h1>Tu espacio.<br><em>Protegido por diseño.</em></h1>
                <p>Actualiza tu contraseña sin perder tus notas, materias, conversaciones ni documentos.</p>
            </div>
            <div class="auth-features" aria-label="Privacidad">
                <div><span class="material-symbols-outlined">shield_locked</span><div><strong>Sesiones bajo control</strong><p>Al guardar se cerrarán tus demás sesiones.</p></div></div>
                <div><span class="material-symbols-outlined">home_storage</span><div><strong>Datos en tu red</strong><p>Tu información permanece en esta instancia local.</p></div></div>
            </div>
            <footer class="auth-intro-foot"><span><i></i> Sistema local disponible</span><span>NOVA · 3.0</span></footer>
        </section>
        <section class="auth-access" aria-label="Cambiar contraseña">
            <div class="auth-card" id="login-container">
                <header class="auth-card-head">
                    <span class="auth-card-icon material-symbols-outlined" aria-hidden="true">key</span>
                    <p>SEGURIDAD DE LA CUENTA</p>
                    <h2>Cambia tu contraseña.</h2>
                    <span>Al guardarla se cerrarán tus demás sesiones.</span>
                </header>
                <!-- Slot de feedback: banner de error (400) o de éxito (200). -->
                <div id="password-feedback" class="mt-5">{banner}</div>
                <form method="post" action="{action}" class="auth-form">
                    <div class="auth-field">
                        <label for="username">Usuario actual</label>
                        {username_input}
                    </div>
                    <div class="auth-field">
                        <label for="current_password">Contraseña actual</label>
                        <div class="auth-input-wrap auth-password">
                            <span class="material-symbols-outlined" aria-hidden="true">lock</span>
                            <input id="current_password" type="password" name="current_password" required autocomplete="current-password" placeholder="Contraseña actual">
                            <button type="button" id="toggle-current_password" aria-label="Mostrar contraseña" aria-pressed="false"><span class="material-symbols-outlined">visibility</span></button>
                        </div>
                    </div>
                    <div class="auth-field">
                        <label for="new_password">Nueva contraseña</label>
                        <div class="auth-input-wrap auth-password">
                            <span class="material-symbols-outlined" aria-hidden="true">key</span>
                            <input id="new_password" type="password" name="new_password" required minlength="{min_len}" autocomplete="new-password" placeholder="Nueva contraseña">
                            <button type="button" id="toggle-new_password" aria-label="Mostrar contraseña" aria-pressed="false"><span class="material-symbols-outlined">visibility</span></button>
                        </div>
                        <small>Mínimo {min_len} caracteres.</small>
                    </div>
                    <div class="auth-field">
                        <label for="confirm_password">Repetir nueva contraseña</label>
                        <div class="auth-input-wrap auth-password">
                            <span class="material-symbols-outlined" aria-hidden="true">check_circle</span>
                            <input id="confirm_password" type="password" name="confirm_password" required minlength="{min_len}" autocomplete="new-password" placeholder="Repite la nueva contraseña">
                            <button type="button" id="toggle-confirm_password" aria-label="Mostrar contraseña" aria-pressed="false"><span class="material-symbols-outlined">visibility</span></button>
                        </div>
                    </div>
                    <button type="submit" class="auth-submit"><span>Actualizar contraseña</span><span class="material-symbols-outlined" aria-hidden="true">arrow_forward</span></button>
                </form>
                <p class="auth-local"><span class="material-symbols-outlined">lock</span> Acceso solo desde la red local</p>
            </div>
            <p class="auth-access-meta">NOVA 3.0 <span>·</span> Tu conocimiento, bajo tu control.</p>
        </section>
    </main>
    <script>{js}</script>
</body>
</html>"##,
        head = head,
        banner = banner,
        username_input = username_input,
        action = PASSWORD_ROUTE,
        min_len = MIN_NEW_PASSWORD_LEN,
        js = PASSWORD_PAGE_JS,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El login exitoso conserva el 302 (lo fijan `tests/auth_sessions.rs` y
    /// `tests/security.rs`) y añade `HX-Redirect` para que htmx navegue de
    /// verdad en vez de inyectar el dashboard dentro de `#login-container`.
    #[test]
    fn login_success_response_keeps_302_and_adds_hx_redirect() {
        let resp = login_success_response(
            "session_id=token-de-prueba; Path=/; HttpOnly; SameSite=Strict; Max-Age=86400",
        );

        assert_eq!(resp.status(), StatusCode::FOUND);
        assert_eq!(
            resp.headers().get("Location").unwrap().to_str().unwrap(),
            "/dashboard"
        );
        assert_eq!(
            resp.headers().get("HX-Redirect").unwrap().to_str().unwrap(),
            "/dashboard"
        );
        assert!(resp.headers().get("set-cookie").is_some());
    }

    /// El registro público está cerrado: sin token de invitación
    /// (`expected = None` ⇔ `REGISTRATION_TOKEN` sin definir en el entorno)
    /// la respuesta es **403** con un motivo honesto — no se crea la cuenta
    /// ni se devuelve un 500.
    #[tokio::test]
    async fn register_sin_token_responde_403() {
        let resp = match registration_gate(None, &HeaderMap::new(), None) {
            Ok(()) => panic!("el registro sin token de invitación debe rechazarse"),
            Err(resp) => resp,
        };

        assert_eq!(resp.status(), StatusCode::FORBIDDEN);

        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .expect("cuerpo de la respuesta 403");
        let text = String::from_utf8_lossy(&body);
        assert!(
            text.contains("El registro está deshabilitado"),
            "mensaje inesperado: {text}"
        );
        assert!(
            text.contains("pide una cuenta al administrador"),
            "mensaje inesperado: {text}"
        );
    }

    /// Reglas de la contraseña nueva: mínimo 12 caracteres, confirmación
    /// coincidente y distinta de la actual.
    #[test]
    fn validate_new_password_rules() {
        let current = "contraseña-actual-de-prueba";
        let nueva = "contraseña-nueva-de-prueba";

        assert!(validate_new_password(nueva, nueva, current).is_ok());
        // Menos de 12 caracteres.
        assert!(validate_new_password("corta", "corta", current).is_err());
        // La confirmación no coincide.
        assert!(validate_new_password(nueva, "otra-cosa-distinta", current).is_err());
        // Igual que la actual.
        assert!(validate_new_password(current, current, current).is_err());
    }

    /// La página de contraseña reutiliza el mismo `head` Stitch que el login
    /// (mismos tokens y clases) y escapa el nombre de usuario que muestra.
    #[test]
    fn password_page_reuses_login_styles_and_escapes_username() {
        let html = password_page_html("<script>alert(1)</script>", false, None);

        assert!(
            html.contains(r#"class="auth-card" id="login-container""#),
            "debe reutilizar la tarjeta NOVA del login"
        );
        assert!(
            html.contains("tailwind.config"),
            "debe reutilizar los tokens Stitch del login"
        );
        assert!(
            !html.contains("<script>alert(1)</script>"),
            "el usuario no debe inyectar HTML"
        );
        assert!(html.contains("&lt;script&gt;"), "el usuario va escapado");
        for campo in ["current_password", "new_password", "confirm_password"] {
            assert!(
                html.contains(&format!("name=\"{}\"", campo)),
                "falta el campo {campo}"
            );
        }
    }
}
