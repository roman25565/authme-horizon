//! The player-facing texts, in Ukrainian and English. The Ukrainian wording follows
//! AuthMeReloaded's `messages_uk.yml` (GPL-3.0, AuthMe team and translators) where AuthMe has
//! the same message.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Msg {
    RegisterRequest,
    LoginRequest,
    RegisterUsage,
    LoginUsage,
    ChangePasswordUsage,
    UnregisterUsage,
    RegisterSuccess,
    LoginSuccess,
    LogoutSuccess,
    PasswordChanged,
    Unregistered,
    WrongPassword,
    WrongPasswordAttempts,
    PasswordMismatch,
    PasswordName,
    PasswordUnsafe,
    PasswordLength,
    PasswordCharacters,
    NameTaken,
    NotRegistered,
    AlreadyLoggedIn,
    NotLoggedIn,
    DeniedCommand,
    DeniedChat,
    Timeout,
    TooManyAttempts,
    Busy,
    Error,
    NameCharacters,
    NameLength,
    NameCase,
    PlayerRegistered,
    LoggedOutByAdmin,
    SessionResumed,
}

/// The languages the messages come in. A new one is a variant here, a code in `parse` and
/// `code`, and a table like `en` below.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    #[default]
    En,
    Uk,
}

impl Language {
    /// `en` or `uk` (also `ua`), any letter case; `None` for anything else.
    pub fn parse(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "en" => Some(Language::En),
            "uk" | "ua" => Some(Language::Uk),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Language::En => "en",
            Language::Uk => "uk",
        }
    }
}

pub struct Lang(pub Language);

impl Lang {
    /// An unknown code falls back to English.
    pub fn new(code: &str) -> Self {
        Lang(Language::parse(code).unwrap_or_default())
    }

    /// `{name}`, `{min}`, `{max}`, `{valid}`, `{left}` are filled from `args`.
    pub fn text(&self, msg: Msg, args: &[(&str, &str)]) -> String {
        let template = match self.0 {
            Language::En => en(msg),
            Language::Uk => uk(msg),
        };
        let mut out = template.to_owned();
        for (k, v) in args {
            out = out.replace(&format!("{{{k}}}"), v);
        }
        out
    }
}

fn uk(m: Msg) -> &'static str {
    use Msg::*;
    match m {
        RegisterRequest => "Перш ніж почати гру, вам потрібно зареєструвати свій нікнейм! Введіть: /register <пароль> <повторПароля>",
        LoginRequest => "Для авторизації введіть команду: /login <пароль>",
        RegisterUsage => "Синтаксис: /register <пароль> <повторПароля>",
        LoginUsage => "Синтаксис: /login <пароль>",
        ChangePasswordUsage => "Синтаксис: /changepassword <старийПароль> <новийПароль>",
        UnregisterUsage => "Синтаксис: /unregister <пароль>",
        RegisterSuccess => "Реєстрація пройшла успішно!",
        LoginSuccess => "Успішна авторизація!",
        LogoutSuccess => "Ви вийшли зі свого акаунта!",
        PasswordChanged => "Пароль успішно змінено!",
        Unregistered => "Дані про реєстрацію успішно видалено.",
        WrongPassword => "Невірний пароль!",
        WrongPasswordAttempts => "Невірний пароль! Залишилось спроб: {left}",
        PasswordMismatch => "Паролі не співпадають!",
        PasswordName => "Не можна використовувати свій нікнейм у якості пароля! Будь ласка, виберіть щось інакше...",
        PasswordUnsafe => "Цей пароль надто простий! Будь ласка, придумайте інакший...",
        PasswordLength => "Пароль має бути від {min} до {max} символів.",
        PasswordCharacters => "Пароль не може містити пробілів і службових символів.",
        NameTaken => "Такий нікнейм вже зареєстровано.",
        NotRegistered => "Цей гравець не є зареєстрованим.",
        AlreadyLoggedIn => "Ви вже авторизовані!",
        NotLoggedIn => "Ви не авторизовані!",
        DeniedCommand => "Для використання цієї команди потрібна авторизація.",
        DeniedChat => "Для доступу до чату потрібна авторизація.",
        Timeout => "Час для авторизації сплинув. Будь ласка, спробуйте ще раз!",
        TooManyAttempts => "Забагато невірних паролів. Спробуйте ще раз пізніше.",
        Busy => "Зачекайте, попередній запит ще обробляється...",
        Error => "[AuthMe] Помилка. Будь ласка, повідомте адміністратора!",
        NameCharacters => "Ваш нікнейм містить недопустимі символи! Дозволено: {valid}",
        NameLength => "Ваш нікнейм надто короткий або надто довгий! (від {min} до {max})",
        NameCase => "Регістр у вашому нікнеймі відрізняється від регістру при реєстрації. Будь ласка, перезайдіть як {valid}",
        PlayerRegistered => "Гравець {name} щойно зареєструвався!",
        LoggedOutByAdmin => "Адміністратор змінив ваш акаунт; будь ласка, авторизуйтесь знову!",
        SessionResumed => "Сесію відновлено: ви авторизовані автоматично (та сама IP-адреса).",
    }
}

fn en(m: Msg) -> &'static str {
    use Msg::*;
    match m {
        RegisterRequest => "Please register to play: /register <password> <confirmPassword>",
        LoginRequest => "Please log in: /login <password>",
        RegisterUsage => "Usage: /register <password> <confirmPassword>",
        LoginUsage => "Usage: /login <password>",
        ChangePasswordUsage => "Usage: /changepassword <oldPassword> <newPassword>",
        UnregisterUsage => "Usage: /unregister <password>",
        RegisterSuccess => "Successfully registered!",
        LoginSuccess => "Successful login!",
        LogoutSuccess => "Logged out successfully!",
        PasswordChanged => "Password changed successfully!",
        Unregistered => "Successfully unregistered.",
        WrongPassword => "Wrong password!",
        WrongPasswordAttempts => "Wrong password! Attempts left: {left}",
        PasswordMismatch => "Passwords didn't match!",
        PasswordName => "You can't use your name as password, please choose another one...",
        PasswordUnsafe => "The chosen password isn't safe, please choose another one...",
        PasswordLength => "Your password must be {min} to {max} characters long.",
        PasswordCharacters => "Your password can't contain spaces or control characters.",
        NameTaken => "You already have registered this username!",
        NotRegistered => "This user isn't registered!",
        AlreadyLoggedIn => "You're already logged in!",
        NotLoggedIn => "You're not logged in!",
        DeniedCommand => "In order to use this command you must be authenticated!",
        DeniedChat => "In order to chat you must be authenticated!",
        Timeout => "Login timeout exceeded, you have been kicked from the server, please try again!",
        TooManyAttempts => "Too many wrong passwords. Try again later.",
        Busy => "Please wait, your last request is still being processed...",
        Error => "[AuthMe] An unexpected error occurred, please contact an administrator!",
        NameCharacters => "Your username contains illegal characters. Allowed: {valid}",
        NameLength => "Your username is either too short or too long! ({min} to {max})",
        NameCase => "You should join using username {valid}",
        PlayerRegistered => "{name} has just registered!",
        LoggedOutByAdmin => "An admin changed your account; please log in again!",
        SessionResumed => "Session resumed: logged in automatically (same IP address).",
    }
}
