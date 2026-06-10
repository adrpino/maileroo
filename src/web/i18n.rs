use axum::{extract::FromRequestParts, http::request::Parts};

#[derive(Debug, Clone, Copy, Default)]
pub enum Locale {
    #[default]
    En,
    Es,
    Fr,
    Pt,
}

impl Locale {
    pub fn as_str(&self) -> &'static str {
        match self {
            Locale::En => "en",
            Locale::Es => "es",
            Locale::Fr => "fr",
            Locale::Pt => "pt",
        }
    }
}

impl<S> FromRequestParts<S> for Locale
where
    S: Send + Sync,
{
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let accept_language = parts
            .headers
            .get(axum::http::header::ACCEPT_LANGUAGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");

        // Simple parser for Accept-Language: es-ES,es;q=0.9,en;q=0.8
        let lang = accept_language.to_lowercase();
        if lang.contains("es") {
            Ok(Locale::Es)
        } else if lang.contains("fr") {
            Ok(Locale::Fr)
        } else if lang.contains("pt") {
            Ok(Locale::Pt)
        } else {
            Ok(Locale::En)
        }
    }
}

pub trait Messages {
    fn login_title(&self) -> &'static str;
    fn email_label(&self) -> &'static str;
    fn password_label(&self) -> &'static str;
    fn login_button(&self) -> &'static str;
    fn register_link_prompt(&self) -> &'static str;
    fn register_link_text(&self) -> &'static str;
    fn invalid_credentials(&self) -> &'static str;
    fn register_title(&self) -> &'static str;
    fn register_button(&self) -> &'static str;
    fn login_link_prompt(&self) -> &'static str;
    fn login_link_text(&self) -> &'static str;
    fn error_invalid_email(&self) -> &'static str;
    fn error_invalid_domain(&self) -> &'static str;
    fn error_email_taken(&self) -> &'static str;
    fn dashboard_title(&self) -> &'static str;
    fn compose_button(&self) -> &'static str;
    fn compose_from(&self) -> &'static str;
    fn compose_to(&self) -> &'static str;
    fn compose_subject(&self) -> &'static str;
    fn compose_body(&self) -> &'static str;
    fn compose_send(&self) -> &'static str;
    fn compose_sending(&self) -> &'static str;
    fn logout_button(&self) -> &'static str;
    fn stats_aliases(&self) -> &'static str;
    fn stats_domains(&self) -> &'static str;
    fn stats_emails(&self) -> &'static str;
    fn manage_aliases(&self) -> &'static str;
    fn manage_domains(&self) -> &'static str;
    fn table_sender(&self) -> &'static str;
    fn table_subject(&self) -> &'static str;
    fn table_received(&self) -> &'static str;
    fn table_forwarded(&self) -> &'static str;
    fn table_actions(&self) -> &'static str;
    fn table_header_to(&self) -> &'static str;
    fn table_header_outbound_status(&self) -> &'static str;
    fn table_header_outbound_sent(&self) -> &'static str;
    fn table_header_outbound_updated(&self) -> &'static str;
    fn no_emails(&self) -> &'static str;
    fn modal_cancel(&self) -> &'static str;
    fn modal_delete_confirm(&self) -> &'static str;
    fn delete_alias_title(&self) -> &'static str;
    fn delete_alias_message(&self, alias: &str) -> String;
    fn delete_email_title(&self) -> &'static str;
    fn delete_email_message(&self, subject: &str) -> String;
    fn error_alias_limit_reached(&self, max: i64) -> String;
    fn error_alias_taken(&self) -> &'static str;
    fn error_alias_reserved(&self) -> &'static str;
    fn error_alias_invalid_format(&self) -> &'static str;
    fn error_alias_too_long(&self, max: usize) -> String;
    fn email_detail_title(&self) -> &'static str;
    fn back_to_dashboard(&self) -> &'static str;
    fn delete_email_button(&self) -> &'static str;
    fn from_label(&self) -> &'static str;
    fn to_label(&self) -> &'static str;
    fn date_label(&self) -> &'static str;
    fn forwarded_label(&self) -> &'static str;
    fn alias_address_header(&self) -> &'static str;
    fn destination_header(&self) -> &'static str;
    fn auto_forward_header(&self) -> &'static str;
    fn create_alias_form_title(&self) -> &'static str;
    fn select_domain_label(&self) -> &'static str;
    fn choose_subdomain_label(&self) -> &'static str;
    fn custom_subdomain_label(&self) -> &'static str;
    fn auto_forward_checkbox_label(&self) -> &'static str;
    fn create_alias_button(&self) -> &'static str;
    fn limit_reached_text(&self, max: &i64) -> String;
    fn copy_tooltip(&self) -> &'static str;
    fn view_button(&self) -> &'static str;
    fn delete_button(&self) -> &'static str;
    fn status_enabled(&self) -> &'static str;
    fn status_disabled(&self) -> &'static str;
    fn status_yes(&self) -> &'static str;
    fn status_no(&self) -> &'static str;
    fn batch_delete_button(&self) -> &'static str;
    fn batch_delete_modal_title(&self) -> &'static str;
    fn batch_delete_modal_message(&self, count: usize) -> String;
    fn recent_emails_title(&self) -> &'static str;
    fn sent_emails_title(&self) -> &'static str;
    fn draft_emails_title(&self) -> &'static str;
    fn folder_inbox(&self) -> &'static str;
    fn folder_sent(&self) -> &'static str;
    fn folder_drafts(&self) -> &'static str;
    fn search_placeholder(&self) -> &'static str;
    fn pagination_previous(&self) -> &'static str;
    fn pagination_next(&self) -> &'static str;
    fn pagination_info(&self, current: &i64, total: &i64) -> String;
    fn from_all_aliases_label(&self) -> &'static str;
    fn filter_all_aliases(&self) -> &'static str;
    fn site_title(&self) -> &'static str;
    fn site_description(&self) -> &'static str;
    fn replies_title(&self) -> &'static str;
    fn reply_form_title(&self) -> &'static str;
    fn reply_placeholder(&self) -> &'static str;
    fn send_reply_button(&self) -> &'static str;
    fn you_replied(&self) -> &'static str;
    fn api_keys_title(&self) -> &'static str;
    fn api_keys_subtitle(&self) -> &'static str;
    fn api_key_success(&self) -> &'static str;
    fn api_key_copy_warning(&self) -> &'static str;
    fn create_key_title(&self) -> &'static str;
    fn key_name_label(&self) -> &'static str;
    fn key_name_placeholder(&self) -> &'static str;
    fn generate_key_button(&self) -> &'static str;
    fn created_on_label(&self, date: &str) -> String;
    fn revoke_button(&self) -> &'static str;
    fn revoke_confirm(&self) -> &'static str;
    fn no_keys_found(&self) -> &'static str;
    fn admin_panel_title(&self) -> &'static str;
    fn user_management_title(&self) -> &'static str;
    fn admin_total_users(&self) -> &'static str;
    fn admin_table_user(&self) -> &'static str;
    fn admin_table_registered(&self) -> &'static str;
    fn admin_table_aliases(&self) -> &'static str;
    fn admin_table_emails(&self) -> &'static str;
    fn admin_table_last_login_ip(&self) -> &'static str;
    fn admin_table_last_login_at(&self) -> &'static str;
    fn admin_table_bypass_limit(&self) -> &'static str;
    fn admin_table_disable_autoclean(&self) -> &'static str;
    fn admin_table_outbound_email(&self) -> &'static str;
    fn admin_bypass_limit_tooltip(&self) -> &'static str;
    fn admin_disable_autoclean_tooltip(&self) -> &'static str;
    fn admin_outbound_email_tooltip(&self) -> &'static str;
    fn admin_badge_admin(&self) -> &'static str;
    fn admin_never(&self) -> &'static str;
    fn load_remote_content(&self) -> &'static str;

    // Toast Notifications
    fn toast_email_sent_success(&self) -> &'static str;
    fn toast_email_send_failed(&self) -> &'static str;
    fn toast_invalid_email(&self) -> &'static str;
    fn toast_empty_subject(&self) -> &'static str;
    fn toast_alias_unauthorized(&self) -> &'static str;

    // DKIM Translations
    fn dkim_modal_title(&self, domain: &str) -> String;
    fn dkim_no_key_warning(&self) -> &'static str;
    fn dkim_no_key_desc(&self) -> &'static str;
    fn dkim_generate_button(&self) -> &'static str;
    fn dkim_pending_title(&self) -> &'static str;
    fn dkim_rotation_active_badge(&self) -> &'static str;
    fn dkim_pending_desc(&self) -> &'static str;
    fn dkim_active_title(&self) -> &'static str;
    fn dkim_active_desc(&self) -> &'static str;
    fn dkim_rotate_button(&self) -> &'static str;
    fn dkim_rotate_confirm(&self) -> &'static str;
    fn dkim_type_label(&self) -> &'static str;
    fn dkim_host_label(&self) -> &'static str;
    fn dkim_value_label(&self) -> &'static str;
    fn dkim_copy_button(&self) -> &'static str;
    fn dkim_ttl_notice(&self) -> &'static str;
    fn dkim_verify_button(&self) -> &'static str;
    fn dkim_cancel_button(&self) -> &'static str;
    fn dkim_cancel_confirm(&self) -> &'static str;
}

impl Messages for Locale {
    fn login_title(&self) -> &'static str {
        match self {
            Locale::En => "Login",
            Locale::Es => "Iniciar Sesión",
            Locale::Fr => "Connexion",
            Locale::Pt => "Entrar",
        }
    }

    fn email_label(&self) -> &'static str {
        match self {
            Locale::En => "Email:",
            Locale::Es => "Correo electrónico:",
            Locale::Fr => "E-mail:",
            Locale::Pt => "E-mail:",
        }
    }

    fn password_label(&self) -> &'static str {
        match self {
            Locale::En => "Password:",
            Locale::Es => "Contraseña:",
            Locale::Fr => "Mot de passe:",
            Locale::Pt => "Senha:",
        }
    }

    fn login_button(&self) -> &'static str {
        match self {
            Locale::En => "Login",
            Locale::Es => "Entrar",
            Locale::Fr => "Se connecter",
            Locale::Pt => "Entrar",
        }
    }

    fn register_link_prompt(&self) -> &'static str {
        match self {
            Locale::En => "Don't have an account?",
            Locale::Es => "¿No tienes una cuenta?",
            Locale::Fr => "Vous n'avez pas de compte ?",
            Locale::Pt => "Não tem uma conta?",
        }
    }

    fn register_link_text(&self) -> &'static str {
        match self {
            Locale::En => "Register here",
            Locale::Es => "Regístrate aquí",
            Locale::Fr => "Inscrivez-vous ici",
            Locale::Pt => "Registe-se aqui",
        }
    }

    fn invalid_credentials(&self) -> &'static str {
        match self {
            Locale::En => "Invalid email or password",
            Locale::Es => "Correo o contraseña inválidos",
            Locale::Fr => "E-mail ou mot de passe invalide",
            Locale::Pt => "E-mail ou senha inválidos",
        }
    }

    fn register_title(&self) -> &'static str {
        match self {
            Locale::En => "Register",
            Locale::Es => "Registro",
            Locale::Fr => "Inscription",
            Locale::Pt => "Registo",
        }
    }

    fn register_button(&self) -> &'static str {
        match self {
            Locale::En => "Register",
            Locale::Es => "Registrarse",
            Locale::Fr => "S'inscrire",
            Locale::Pt => "Registar",
        }
    }

    fn login_link_prompt(&self) -> &'static str {
        match self {
            Locale::En => "Already have an account?",
            Locale::Es => "¿Ya tienes una cuenta?",
            Locale::Fr => "Vous avez déjà un compte ?",
            Locale::Pt => "Já tem uma conta?",
        }
    }

    fn login_link_text(&self) -> &'static str {
        match self {
            Locale::En => "Login here",
            Locale::Es => "Inicia sesión aquí",
            Locale::Fr => "Connectez-vous ici",
            Locale::Pt => "Entre aqui",
        }
    }

    fn error_invalid_email(&self) -> &'static str {
        match self {
            Locale::En => "Invalid email",
            Locale::Es => "Correo inválido",
            Locale::Fr => "E-mail invalide",
            Locale::Pt => "E-mail inválido",
        }
    }

    fn error_invalid_domain(&self) -> &'static str {
        match self {
            Locale::En => "Invalid domain",
            Locale::Es => "Dominio inválido",
            Locale::Fr => "Domaine invalide",
            Locale::Pt => "Domínio inválido",
        }
    }

    fn error_email_taken(&self) -> &'static str {
        match self {
            Locale::En => "Email already registered",
            Locale::Es => "Este correo ya está registrado",
            Locale::Fr => "E-mail déjà enregistré",
            Locale::Pt => "E-mail já registado",
        }
    }

    fn dashboard_title(&self) -> &'static str {
        match self {
            Locale::En => "Dashboard",
            Locale::Es => "Panel de Control",
            Locale::Fr => "Tableau de bord",
            Locale::Pt => "Painel de Controlo",
        }
    }

    fn compose_button(&self) -> &'static str {
        match self {
            Locale::En => "Compose",
            Locale::Es => "Redactar",
            Locale::Fr => "Nouveau message",
            Locale::Pt => "Compor",
        }
    }

    fn compose_from(&self) -> &'static str {
        match self {
            Locale::En => "From",
            Locale::Es => "De",
            Locale::Fr => "De",
            Locale::Pt => "De",
        }
    }

    fn compose_to(&self) -> &'static str {
        match self {
            Locale::En => "To",
            Locale::Es => "Para",
            Locale::Fr => "À",
            Locale::Pt => "Para",
        }
    }

    fn compose_subject(&self) -> &'static str {
        match self {
            Locale::En => "Subject",
            Locale::Es => "Asunto",
            Locale::Fr => "Sujet",
            Locale::Pt => "Assunto",
        }
    }

    fn compose_body(&self) -> &'static str {
        match self {
            Locale::En => "Message",
            Locale::Es => "Mensaje",
            Locale::Fr => "Message",
            Locale::Pt => "Mensagem",
        }
    }

    fn compose_send(&self) -> &'static str {
        match self {
            Locale::En => "Send Email",
            Locale::Es => "Enviar Correo",
            Locale::Fr => "Envoyer",
            Locale::Pt => "Enviar Email",
        }
    }

    fn compose_sending(&self) -> &'static str {
        match self {
            Locale::En => "Sending...",
            Locale::Es => "Enviando...",
            Locale::Fr => "Envoi en cours...",
            Locale::Pt => "A enviar...",
        }
    }

    fn logout_button(&self) -> &'static str {
        match self {
            Locale::En => "Logout",
            Locale::Es => "Cerrar Sesión",
            Locale::Fr => "Déconnexion",
            Locale::Pt => "Sair",
        }
    }

    fn stats_aliases(&self) -> &'static str {
        match self {
            Locale::En => "Active Aliases",
            Locale::Es => "Alias Activos",
            Locale::Fr => "Alias actifs",
            Locale::Pt => "Alias Ativos",
        }
    }

    fn stats_domains(&self) -> &'static str {
        match self {
            Locale::En => "Connected Domains",
            Locale::Es => "Dominios Conectados",
            Locale::Fr => "Domaines connectés",
            Locale::Pt => "Domínios Conectados",
        }
    }

    fn stats_emails(&self) -> &'static str {
        match self {
            Locale::En => "Total Emails Received",
            Locale::Es => "Correos Recibidos",
            Locale::Fr => "Total des e-mails reçus",
            Locale::Pt => "Total de E-mails Recebidos",
        }
    }

    fn manage_aliases(&self) -> &'static str {
        match self {
            Locale::En => "Manage Aliases",
            Locale::Es => "Gestionar Alias",
            Locale::Fr => "Gérer les alias",
            Locale::Pt => "Gerir alias",
        }
    }

    fn manage_domains(&self) -> &'static str {
        match self {
            Locale::En => "Manage Domains",
            Locale::Es => "Gestionar Dominios",
            Locale::Fr => "Gérer les domaines",
            Locale::Pt => "Gerir domínios",
        }
    }

    fn table_sender(&self) -> &'static str {
        match self {
            Locale::En => "Sender",
            Locale::Es => "Remitente",
            Locale::Fr => "Expéditeur",
            Locale::Pt => "Remetente",
        }
    }

    fn table_subject(&self) -> &'static str {
        match self {
            Locale::En => "Subject",
            Locale::Es => "Asunto",
            Locale::Fr => "Objet",
            Locale::Pt => "Assunto",
        }
    }

    fn table_received(&self) -> &'static str {
        match self {
            Locale::En => "Received At",
            Locale::Es => "Recibido el",
            Locale::Fr => "Reçu le",
            Locale::Pt => "Recebido em",
        }
    }

    fn table_forwarded(&self) -> &'static str {
        match self {
            Locale::En => "Forwarded",
            Locale::Es => "Reenviado",
            Locale::Fr => "Transféré",
            Locale::Pt => "Reencaminhado",
        }
    }

    fn table_header_to(&self) -> &'static str {
        match self {
            Locale::En => "Recipient",
            Locale::Es => "Destinatario",
            Locale::Fr => "Destinataire",
            Locale::Pt => "Destinatário",
        }
    }

    fn table_header_outbound_status(&self) -> &'static str {
        match self {
            Locale::En => "Status",
            Locale::Es => "Estado",
            Locale::Fr => "Statut",
            Locale::Pt => "Estado",
        }
    }

    fn table_header_outbound_sent(&self) -> &'static str {
        match self {
            Locale::En => "Sent At",
            Locale::Es => "Enviado el",
            Locale::Fr => "Envoyé le",
            Locale::Pt => "Enviado em",
        }
    }

    fn table_header_outbound_updated(&self) -> &'static str {
        match self {
            Locale::En => "Last Updated",
            Locale::Es => "Última Edición",
            Locale::Fr => "Dernière modification",
            Locale::Pt => "Última Edição",
        }
    }

    fn table_actions(&self) -> &'static str {
        match self {
            Locale::En => "Actions",
            Locale::Es => "Acciones",
            Locale::Fr => "Actions",
            Locale::Pt => "Ações",
        }
    }

    fn no_emails(&self) -> &'static str {
        match self {
            Locale::En => "No emails received yet.",
            Locale::Es => "No se han recibido correos aún.",
            Locale::Fr => "Aucun e-mail reçu pour le moment.",
            Locale::Pt => "Nenhum e-mail recebido ainda.",
        }
    }

    fn recent_emails_title(&self) -> &'static str {
        match self {
            Locale::En => "Recent Emails",
            Locale::Es => "Correos Recientes",
            Locale::Fr => "E-mails récents",
            Locale::Pt => "E-mails Recentes",
        }
    }

    fn sent_emails_title(&self) -> &'static str {
        match self {
            Locale::En => "Sent Emails",
            Locale::Es => "Correos Enviados",
            Locale::Fr => "E-mails envoyés",
            Locale::Pt => "E-mails Enviados",
        }
    }

    fn draft_emails_title(&self) -> &'static str {
        match self {
            Locale::En => "Draft Emails",
            Locale::Es => "Borradores",
            Locale::Fr => "Brouillons",
            Locale::Pt => "Rascunhos",
        }
    }

    fn folder_inbox(&self) -> &'static str {
        match self {
            Locale::En => "Inbox",
            Locale::Es => "Bandeja de Entrada",
            Locale::Fr => "Boîte de réception",
            Locale::Pt => "Caixa de Entrada",
        }
    }

    fn folder_sent(&self) -> &'static str {
        match self {
            Locale::En => "Sent",
            Locale::Es => "Enviados",
            Locale::Fr => "Envoyés",
            Locale::Pt => "Enviados",
        }
    }

    fn folder_drafts(&self) -> &'static str {
        match self {
            Locale::En => "Drafts",
            Locale::Es => "Borradores",
            Locale::Fr => "Brouillons",
            Locale::Pt => "Rascunhos",
        }
    }

    fn search_placeholder(&self) -> &'static str {
        match self {
            Locale::En => "Search emails...",
            Locale::Es => "Buscar correos...",
            Locale::Fr => "Rechercher des emails...",
            Locale::Pt => "Pesquisar e-mails...",
        }
    }

    fn pagination_previous(&self) -> &'static str {
        match self {
            Locale::En => "Previous",
            Locale::Es => "Anterior",
            Locale::Fr => "Précédent",
            Locale::Pt => "Anterior",
        }
    }

    fn pagination_next(&self) -> &'static str {
        match self {
            Locale::En => "Next",
            Locale::Es => "Siguiente",
            Locale::Fr => "Suivant",
            Locale::Pt => "Seguinte",
        }
    }

    fn pagination_info(&self, current: &i64, total: &i64) -> String {
        match self {
            Locale::En => format!("Page {} of {}", current, total),
            Locale::Es => format!("Página {} de {}", current, total),
            Locale::Fr => format!("Page {} sur {}", current, total),
            Locale::Pt => format!("Página {} de {}", current, total),
        }
    }

    fn from_all_aliases_label(&self) -> &'static str {
        match self {
            Locale::En => "From all aliases",
            Locale::Es => "De todos los alias",
            Locale::Fr => "De tous les alias",
            Locale::Pt => "De todos os alias",
        }
    }

    fn filter_all_aliases(&self) -> &'static str {
        match self {
            Locale::En => "All Aliases",
            Locale::Es => "Todos los Alias",
            Locale::Fr => "Tous les alias",
            Locale::Pt => "Todos os Alias",
        }
    }

    fn site_title(&self) -> &'static str {
        match self {
            Locale::En => "Maileroo - Email Forwarder for Privacy",
            Locale::Es => "Maileroo - Reenvío de Email para Privacidad",
            Locale::Fr => "Maileroo - Redirection d'emails pour la confidentialité",
            Locale::Pt => "Maileroo - Redirecionamento de Email para Privacidade",
        }
    }

    fn site_description(&self) -> &'static str {
        match self {
            Locale::En => {
                "Protect your identity with temporary and permanent email aliases. Forward emails securely and stay private."
            }
            Locale::Es => {
                "Protege tu identidad con alias de email temporales y permanentes. Reenvía correos de forma segura y mantén tu privacidad."
            }
            Locale::Fr => {
                "Protégez votre identité avec des alias d'e-mail temporaires et permanents. Redirigez vos e-mails en toute sécurité et restez anonyme."
            }
            Locale::Pt => {
                "Proteja a sua identidade com alias de e-mail temporários e permanentes. Redirecione e-mails de forma segura e mantenha a sua privacidade."
            }
        }
    }

    fn replies_title(&self) -> &'static str {
        match self {
            Locale::En => "Replies",
            Locale::Es => "Respuestas",
            Locale::Fr => "Réponses",
            Locale::Pt => "Respostas",
        }
    }

    fn reply_form_title(&self) -> &'static str {
        match self {
            Locale::En => "Send a Reply",
            Locale::Es => "Enviar una Respuesta",
            Locale::Fr => "Envoyer une réponse",
            Locale::Pt => "Enviar uma Resposta",
        }
    }

    fn reply_placeholder(&self) -> &'static str {
        match self {
            Locale::En => "Type your reply here...",
            Locale::Es => "Escribe tu respuesta aquí...",
            Locale::Fr => "Tapez votre réponse ici...",
            Locale::Pt => "Digite sua resposta aqui...",
        }
    }

    fn send_reply_button(&self) -> &'static str {
        match self {
            Locale::En => "Send Reply",
            Locale::Es => "Enviar Respuesta",
            Locale::Fr => "Envoyer la réponse",
            Locale::Pt => "Enviar Resposta",
        }
    }

    fn you_replied(&self) -> &'static str {
        match self {
            Locale::En => "You replied",
            Locale::Es => "Respondiste",
            Locale::Fr => "Vous avez répondu",
            Locale::Pt => "Você respondeu",
        }
    }

    fn api_keys_title(&self) -> &'static str {
        match self {
            Locale::En => "API Keys",
            Locale::Es => "Claves de API",
            Locale::Fr => "Clés API",
            Locale::Pt => "Chaves de API",
        }
    }

    fn api_keys_subtitle(&self) -> &'static str {
        match self {
            Locale::En => "Manage your API keys for programmatic access to Maily.",
            Locale::Es => "Gestiona tus claves de API para el acceso programático a Maily.",
            Locale::Fr => "Gérez vos clés API pour un accès programmatique à Maily.",
            Locale::Pt => "Gira as suas chaves de API para acesso programático ao Maily.",
        }
    }

    fn api_key_success(&self) -> &'static str {
        match self {
            Locale::En => "Success!",
            Locale::Es => "¡Éxito!",
            Locale::Fr => "Succès !",
            Locale::Pt => "Sucesso!",
        }
    }

    fn api_key_copy_warning(&self) -> &'static str {
        match self {
            Locale::En => {
                "Your new API key has been generated. Copy it now - you won't be able to see it again!"
            }
            Locale::Es => {
                "Tu nueva clave de API ha sido generada. Copiala ahora - ¡no podrás volver a verla!"
            }
            Locale::Fr => {
                "Votre nouvelle clé API a été générée. Copiez-la maintenant - vous ne pourrez plus la revoir !"
            }
            Locale::Pt => {
                "A sua nova chave de API foi gerada. Copie-a agora - não poderá vê-la novamente!"
            }
        }
    }

    fn create_key_title(&self) -> &'static str {
        match self {
            Locale::En => "Create New Key",
            Locale::Es => "Crear Nueva Clave",
            Locale::Fr => "Créer une nouvelle clé",
            Locale::Pt => "Criar Nova Chave",
        }
    }

    fn key_name_label(&self) -> &'static str {
        match self {
            Locale::En => "Key Name",
            Locale::Es => "Nombre de la Clave",
            Locale::Fr => "Nom de la clé",
            Locale::Pt => "Nome da Chave",
        }
    }

    fn key_name_placeholder(&self) -> &'static str {
        match self {
            Locale::En => "e.g. CI/CD Pipeline",
            Locale::Es => "ej. CI/CD Pipeline",
            Locale::Fr => "ex. Pipeline CI/CD",
            Locale::Pt => "ex. Pipeline CI/CD",
        }
    }

    fn generate_key_button(&self) -> &'static str {
        match self {
            Locale::En => "Generate API Key",
            Locale::Es => "Generar Clave de API",
            Locale::Fr => "Générer la clé API",
            Locale::Pt => "Gerar Chave de API",
        }
    }

    fn created_on_label(&self, date: &str) -> String {
        match self {
            Locale::En => format!("Created on {}", date),
            Locale::Es => format!("Creada el {}", date),
            Locale::Fr => format!("Créée le {}", date),
            Locale::Pt => format!("Criada em {}", date),
        }
    }

    fn revoke_button(&self) -> &'static str {
        match self {
            Locale::En => "Revoke",
            Locale::Es => "Revocar",
            Locale::Fr => "Révoquer",
            Locale::Pt => "Revogar",
        }
    }

    fn revoke_confirm(&self) -> &'static str {
        match self {
            Locale::En => "Are you sure you want to revoke this API key?",
            Locale::Es => "¿Estás seguro de que quieres revocar esta clave de API?",
            Locale::Fr => "Êtes-vous sûr de vouloir révoquer cette clé API ?",
            Locale::Pt => "Tem a certeza que deseja revogar esta chave de API?",
        }
    }

    fn no_keys_found(&self) -> &'static str {
        match self {
            Locale::En => "No API keys found.",
            Locale::Es => "No se encontraron claves de API.",
            Locale::Fr => "Aucune clé API trouvée.",
            Locale::Pt => "Nenhuma chave de API encontrada.",
        }
    }

    fn modal_cancel(&self) -> &'static str {
        match self {
            Locale::En => "Cancel",
            Locale::Es => "Cancelar",
            Locale::Fr => "Annuler",
            Locale::Pt => "Cancelar",
        }
    }

    fn modal_delete_confirm(&self) -> &'static str {
        match self {
            Locale::En => "Yes, Delete",
            Locale::Es => "Sí, Eliminar",
            Locale::Fr => "Oui, supprimer",
            Locale::Pt => "Sim, Eliminar",
        }
    }

    fn delete_alias_title(&self) -> &'static str {
        match self {
            Locale::En => "Delete Alias",
            Locale::Es => "Eliminar Alias",
            Locale::Fr => "Supprimer l'alias",
            Locale::Pt => "Eliminar Alias",
        }
    }

    fn delete_alias_message(&self, alias: &str) -> String {
        let clean_alias = ammonia::clean_text(alias);
        match self {
            Locale::En => format!(
                "Are you sure you want to delete the alias <strong>{}</strong>?<br><br>You will no longer receive emails that are sent to this alias (and other users may use this address)",
                clean_alias
            ),
            Locale::Es => format!(
                "¿Estás seguro de que quieres eliminar el alias <strong>{}</strong>?<br><br>Ya no recibirás los correos enviados a este alias (y otros usuarios podrán usar esta dirección)",
                clean_alias
            ),
            Locale::Fr => format!(
                "Êtes-vous sûr de vouloir supprimer l'alias <strong>{}</strong> ?<br><br>Vous ne recevrez plus les e-mails envoyés à cet alias (et d'autres utilisateurs pourront utiliser cette adresse)",
                clean_alias
            ),
            Locale::Pt => format!(
                "Tem a certeza que deseja eliminar o alias <strong>{}</strong>?<br><br>Deixará de receber e-mails enviados para este alias (e outros utilizadores poderão utilizar este endereço)",
                clean_alias
            ),
        }
    }

    fn delete_email_title(&self) -> &'static str {
        match self {
            Locale::En => "Delete Email",
            Locale::Es => "Eliminar Correo",
            Locale::Fr => "Supprimer l'e-mail",
            Locale::Pt => "Eliminar E-mail",
        }
    }

    fn delete_email_message(&self, subject: &str) -> String {
        let clean_subject = ammonia::clean_text(subject);
        match self {
            Locale::En => format!(
                "Are you sure you want to delete the email with subject: <strong>{}</strong>?",
                clean_subject
            ),
            Locale::Es => format!(
                "¿Estás seguro de que quieres eliminar el correo con asunto: <strong>{}</strong>?",
                clean_subject
            ),
            Locale::Fr => format!(
                "Êtes-vous sûr de vouloir supprimer l'e-mail avec l'objet : <strong>{}</strong> ?",
                clean_subject
            ),
            Locale::Pt => format!(
                "Tem a certeza que deseja eliminar o e-mail com o assunto: <strong>{}</strong>?",
                clean_subject
            ),
        }
    }

    fn error_alias_limit_reached(&self, max: i64) -> String {
        match self {
            Locale::En => format!("You have reached the maximum limit of {} aliases.", max),
            Locale::Es => format!("Has alcanzado el límite máximo de {} alias.", max),
            Locale::Fr => format!("Vous avez atteint la limite maximale de {} alias.", max),
            Locale::Pt => format!("Atingiu o limite máximo de {} alias.", max),
        }
    }

    fn error_alias_taken(&self) -> &'static str {
        match self {
            Locale::En => "Sorry, this alias was just taken. Please choose another one.",
            Locale::Es => "Lo sentimos, este alias acaba de ser ocupado. Por favor elige otro.",
            Locale::Fr => "Désolé, cet alias vient d'être pris. Veuillez en choisir un autre.",
            Locale::Pt => "Desculpe, este alias já foi ocupado. Por favor, escolha outro.",
        }
    }

    fn error_alias_reserved(&self) -> &'static str {
        match self {
            Locale::En => "This alias is reserved and cannot be registered.",
            Locale::Es => "Este alias está reservado y no puede ser registrado.",
            Locale::Fr => "Cet alias est réservé et ne peut pas être enregistré.",
            Locale::Pt => "Este alias está reservado e não pode ser registado.",
        }
    }

    fn error_alias_invalid_format(&self) -> &'static str {
        match self {
            Locale::En => "Alias can only contain lowercase letters, numbers, hyphens, and dots.",
            Locale::Es => {
                "El alias solo puede contener letras minúsculas, números, guiones y puntos."
            }
            Locale::Fr => {
                "L'alias ne peut contenir que des lettres minuscules, des chiffres, des tirets et des points."
            }
            Locale::Pt => "O alias só pode conter letras minúsculas, números, hífenes e pontos.",
        }
    }

    fn error_alias_too_long(&self, max: usize) -> String {
        match self {
            Locale::En => format!("Alias name is too long (maximum {} characters).", max),
            Locale::Es => format!(
                "El nombre del alias es demasiado largo (máximo {} caracteres).",
                max
            ),
            Locale::Fr => format!(
                "Le nom de l'alias est trop long (maximum {} caractères).",
                max
            ),
            Locale::Pt => format!(
                "O nome do alias é demasiado longo (máximo de {} caracteres).",
                max
            ),
        }
    }

    fn email_detail_title(&self) -> &'static str {
        match self {
            Locale::En => "Email Detail",
            Locale::Es => "Detalle del Correo",
            Locale::Fr => "Détails de l'e-mail",
            Locale::Pt => "Detalhe do E-mail",
        }
    }

    fn back_to_dashboard(&self) -> &'static str {
        match self {
            Locale::En => "Back to Dashboard",
            Locale::Es => "Volver al Panel",
            Locale::Fr => "Retour au tableau de bord",
            Locale::Pt => "Voltar ao Painel",
        }
    }

    fn delete_email_button(&self) -> &'static str {
        match self {
            Locale::En => "Delete Email",
            Locale::Es => "Eliminar Correo",
            Locale::Fr => "Supprimer l'e-mail",
            Locale::Pt => "Eliminar E-mail",
        }
    }

    fn from_label(&self) -> &'static str {
        match self {
            Locale::En => "From:",
            Locale::Es => "De:",
            Locale::Fr => "De :",
            Locale::Pt => "De:",
        }
    }

    fn to_label(&self) -> &'static str {
        match self {
            Locale::En => "To:",
            Locale::Es => "Para:",
            Locale::Fr => "À :",
            Locale::Pt => "Para:",
        }
    }

    fn date_label(&self) -> &'static str {
        match self {
            Locale::En => "Date:",
            Locale::Es => "Fecha:",
            Locale::Fr => "Date :",
            Locale::Pt => "Data:",
        }
    }

    fn forwarded_label(&self) -> &'static str {
        match self {
            Locale::En => "Forwarded:",
            Locale::Es => "Reenviado:",
            Locale::Fr => "Redirigé :",
            Locale::Pt => "Reencaminhado:",
        }
    }

    fn alias_address_header(&self) -> &'static str {
        match self {
            Locale::En => "Alias Address",
            Locale::Es => "Dirección del Alias",
            Locale::Fr => "Adresse de l'alias",
            Locale::Pt => "Endereço do Alias",
        }
    }

    fn destination_header(&self) -> &'static str {
        match self {
            Locale::En => "Destination",
            Locale::Es => "Destino",
            Locale::Fr => "Destination",
            Locale::Pt => "Destino",
        }
    }

    fn auto_forward_header(&self) -> &'static str {
        match self {
            Locale::En => "Auto-Forward",
            Locale::Es => "Reenvío Automático",
            Locale::Fr => "Redirection auto",
            Locale::Pt => "Redirecionamento Automático",
        }
    }

    fn create_alias_form_title(&self) -> &'static str {
        match self {
            Locale::En => "Create New Corporate Alias",
            Locale::Es => "Crear Nuevo Alias Corporativo",
            Locale::Fr => "Créer un nouvel alias d'entreprise",
            Locale::Pt => "Criar Novo Alias Corporativo",
        }
    }

    fn select_domain_label(&self) -> &'static str {
        match self {
            Locale::En => "Select Domain",
            Locale::Es => "Seleccionar Dominio",
            Locale::Fr => "Sélectionner le domaine",
            Locale::Pt => "Selecionar Domínio",
        }
    }

    fn choose_subdomain_label(&self) -> &'static str {
        match self {
            Locale::En => "Choose a Subdomain",
            Locale::Es => "Elegir un Subdominio",
            Locale::Fr => "Choisir un sous-domaine",
            Locale::Pt => "Escolher um Subdomínio",
        }
    }

    fn custom_subdomain_label(&self) -> &'static str {
        match self {
            Locale::En => "Or enter custom",
            Locale::Es => "O introducir personalizado",
            Locale::Fr => "Ou saisir un nom personnalisé",
            Locale::Pt => "Ou introduzir personalizado",
        }
    }

    fn auto_forward_checkbox_label(&self) -> &'static str {
        match self {
            Locale::En => "Auto-forward emails to my inbox",
            Locale::Es => "Reenviar correos a mi bandeja de entrada",
            Locale::Fr => "Rediriger les e-mails vers ma boîte de réception",
            Locale::Pt => "Redirecionar e-mails para a minha caixa de entrada",
        }
    }

    fn create_alias_button(&self) -> &'static str {
        match self {
            Locale::En => "Create Alias",
            Locale::Es => "Crear Alias",
            Locale::Fr => "Créer l'alias",
            Locale::Pt => "Criar Alias",
        }
    }

    fn limit_reached_text(&self, max: &i64) -> String {
        match self {
            Locale::En => format!("Limit of {} aliases reached.", max),
            Locale::Es => format!("Límite de {} alias alcanzado.", max),
            Locale::Fr => format!("Limite de {} alias atteinte.", max),
            Locale::Pt => format!("Limite de {} alias atingido.", max),
        }
    }

    fn copy_tooltip(&self) -> &'static str {
        match self {
            Locale::En => "Copy to clipboard",
            Locale::Es => "Copiar al portapapeles",
            Locale::Fr => "Copier dans le presse-papiers",
            Locale::Pt => "Copiar para a área de transferência",
        }
    }

    fn view_button(&self) -> &'static str {
        match self {
            Locale::En => "View",
            Locale::Es => "Ver",
            Locale::Fr => "Voir",
            Locale::Pt => "Ver",
        }
    }

    fn delete_button(&self) -> &'static str {
        match self {
            Locale::En => "Delete",
            Locale::Es => "Eliminar",
            Locale::Fr => "Supprimer",
            Locale::Pt => "Eliminar",
        }
    }

    fn status_enabled(&self) -> &'static str {
        match self {
            Locale::En => "Enabled",
            Locale::Es => "Activado",
            Locale::Fr => "Activé",
            Locale::Pt => "Ativado",
        }
    }

    fn status_disabled(&self) -> &'static str {
        match self {
            Locale::En => "Disabled",
            Locale::Es => "Desactivado",
            Locale::Fr => "Désactivé",
            Locale::Pt => "Desativado",
        }
    }

    fn status_yes(&self) -> &'static str {
        match self {
            Locale::En => "Yes",
            Locale::Es => "Sí",
            Locale::Fr => "Oui",
            Locale::Pt => "Sim",
        }
    }

    fn status_no(&self) -> &'static str {
        match self {
            Locale::En => "No",
            Locale::Es => "No",
            Locale::Fr => "Non",
            Locale::Pt => "Não",
        }
    }

    fn batch_delete_button(&self) -> &'static str {
        match self {
            Locale::En => "Delete Selected",
            Locale::Es => "Eliminar Seleccionados",
            Locale::Fr => "Supprimer la sélection",
            Locale::Pt => "Eliminar Selecionados",
        }
    }

    fn batch_delete_modal_title(&self) -> &'static str {
        match self {
            Locale::En => "Delete Multiple Emails",
            Locale::Es => "Eliminar Múltiples Correos",
            Locale::Fr => "Supprimer plusieurs e-mails",
            Locale::Pt => "Eliminar Múltiplos E-mails",
        }
    }

    fn batch_delete_modal_message(&self, count: usize) -> String {
        match self {
            Locale::En => format!(
                "Are you sure you want to delete <strong>{}</strong> selected emails?",
                count
            ),
            Locale::Es => format!(
                "¿Estás seguro de que quieres eliminar <strong>{}</strong> correos seleccionados?",
                count
            ),
            Locale::Fr => format!(
                "Êtes-vous sûr de vouloir supprimer <strong>{}</strong> e-mails sélectionnés ?",
                count
            ),
            Locale::Pt => format!(
                "Tem a certeza que deseja eliminar <strong>{}</strong> e-mails selecionados?",
                count
            ),
        }
    }

    fn admin_panel_title(&self) -> &'static str {
        match self {
            Locale::En => "Admin Panel",
            Locale::Es => "Panel de Administración",
            Locale::Fr => "Panneau d'administration",
            Locale::Pt => "Painel de Administração",
        }
    }

    fn user_management_title(&self) -> &'static str {
        match self {
            Locale::En => "User Management",
            Locale::Es => "Gestión de Usuarios",
            Locale::Fr => "Gestion des Utilisateurs",
            Locale::Pt => "Gestão de Utilizadores",
        }
    }

    fn admin_total_users(&self) -> &'static str {
        match self {
            Locale::En => "Total Users",
            Locale::Es => "Usuarios Totales",
            Locale::Fr => "Total des utilisateurs",
            Locale::Pt => "Total de Utilizadores",
        }
    }

    fn admin_table_user(&self) -> &'static str {
        match self {
            Locale::En => "User / Email",
            Locale::Es => "Usuario / Correo",
            Locale::Fr => "Utilisateur / E-mail",
            Locale::Pt => "Utilizador / E-mail",
        }
    }

    fn admin_table_registered(&self) -> &'static str {
        match self {
            Locale::En => "Registered",
            Locale::Es => "Registrado",
            Locale::Fr => "Inscrit",
            Locale::Pt => "Registado",
        }
    }

    fn admin_table_aliases(&self) -> &'static str {
        match self {
            Locale::En => "Aliases",
            Locale::Es => "Alias",
            Locale::Fr => "Alias",
            Locale::Pt => "Alias",
        }
    }

    fn admin_table_emails(&self) -> &'static str {
        match self {
            Locale::En => "Emails",
            Locale::Es => "Correos",
            Locale::Fr => "E-mails",
            Locale::Pt => "E-mails",
        }
    }

    fn admin_table_last_login_ip(&self) -> &'static str {
        match self {
            Locale::En => "Last Login IP",
            Locale::Es => "IP Último Ingreso",
            Locale::Fr => "Dernière IP de connexion",
            Locale::Pt => "IP Último Início",
        }
    }

    fn admin_table_last_login_at(&self) -> &'static str {
        match self {
            Locale::En => "Last Login At",
            Locale::Es => "Último Ingreso el",
            Locale::Fr => "Dernière connexion le",
            Locale::Pt => "Último Início em",
        }
    }

    fn admin_table_bypass_limit(&self) -> &'static str {
        match self {
            Locale::En => "Bypass Limit",
            Locale::Es => "Ignorar Límite",
            Locale::Fr => "Ignorer la Limite",
            Locale::Pt => "Ignorar Limite",
        }
    }

    fn admin_table_disable_autoclean(&self) -> &'static str {
        match self {
            Locale::En => "Keep All Emails",
            Locale::Es => "Conservar Emails",
            Locale::Fr => "Conserver Emails",
            Locale::Pt => "Manter Emails",
        }
    }

    fn admin_table_outbound_email(&self) -> &'static str {
        match self {
            Locale::En => "Outbound Email",
            Locale::Es => "Envío de Correos",
            Locale::Fr => "Envoi d'E-mails",
            Locale::Pt => "Envio de Emails",
        }
    }

    fn admin_bypass_limit_tooltip(&self) -> &'static str {
        match self {
            Locale::En => "Toggle to bypass the maximum aliases limit for this user",
            Locale::Es => "Alternar para ignorar el límite de alias máximos para este usuario",
            Locale::Fr => "Basculer pour ignorer la limite d'alias maximum pour cet utilisateur",
            Locale::Pt => "Alternar para ignorar o limite de aliases para este utilizador",
        }
    }

    fn admin_disable_autoclean_tooltip(&self) -> &'static str {
        match self {
            Locale::En => {
                "Toggle to prevent emails in this account from being automatically deleted based on retention settings."
            }
            Locale::Es => {
                "Alternar para evitar que los correos de esta cuenta sean eliminados automáticamente."
            }
            Locale::Fr => {
                "Basculer pour empêcher la suppression automatique des emails de ce compte."
            }
            Locale::Pt => {
                "Alternar para impedir que os emails desta conta sejam eliminados automaticamente."
            }
        }
    }

    fn admin_outbound_email_tooltip(&self) -> &'static str {
        match self {
            Locale::En => "Toggle to allow this user to send outbound emails.",
            Locale::Es => "Alternar para permitir que este usuario envíe correos electrónicos.",
            Locale::Fr => "Basculer pour permettre à cet utilisateur d'envoyer des e-mails.",
            Locale::Pt => "Alternar para permitir que este utilizador envie emails.",
        }
    }

    fn admin_badge_admin(&self) -> &'static str {
        match self {
            Locale::En => "ADMIN",
            Locale::Es => "ADMIN",
            Locale::Fr => "ADMIN",
            Locale::Pt => "ADMIN",
        }
    }

    fn admin_never(&self) -> &'static str {
        match self {
            Locale::En => "Never",
            Locale::Es => "Nunca",
            Locale::Fr => "Jamais",
            Locale::Pt => "Nunca",
        }
    }

    fn load_remote_content(&self) -> &'static str {
        match self {
            Locale::En => "Load Remote Content",
            Locale::Es => "Cargar contenido remoto",
            Locale::Fr => "Charger le contenu distant",
            Locale::Pt => "Carregar Conteúdo Remoto",
        }
    }

    fn toast_email_sent_success(&self) -> &'static str {
        match self {
            Locale::En => "Email sent successfully!",
            Locale::Es => "¡Correo enviado con éxito!",
            Locale::Fr => "E-mail envoyé avec succès !",
            Locale::Pt => "E-mail enviado com sucesso!",
        }
    }

    fn toast_email_send_failed(&self) -> &'static str {
        match self {
            Locale::En => "Failed to send email. Check logs.",
            Locale::Es => "Error al enviar correo. Revise los logs.",
            Locale::Fr => "Échec de l'envoi. Vérifiez les journaux.",
            Locale::Pt => "Falha ao enviar e-mail. Verifique os logs.",
        }
    }

    fn toast_invalid_email(&self) -> &'static str {
        match self {
            Locale::En => "Invalid recipient email address.",
            Locale::Es => "Dirección de correo destinatario no válida.",
            Locale::Fr => "Adresse e-mail du destinataire invalide.",
            Locale::Pt => "Endereço de e-mail do destinatário inválido.",
        }
    }

    fn toast_empty_subject(&self) -> &'static str {
        match self {
            Locale::En => "Subject cannot be empty.",
            Locale::Es => "El asunto no puede estar vacío.",
            Locale::Fr => "Le sujet ne peut pas être vide.",
            Locale::Pt => "O assunto não pode estar vazio.",
        }
    }

    fn toast_alias_unauthorized(&self) -> &'static str {
        match self {
            Locale::En => "Alias not found or you do not have permission to use it.",
            Locale::Es => "Alias no encontrado o no tiene permisos para usarlo.",
            Locale::Fr => "Alias introuvable ou vous n'avez pas la permission de l'utiliser.",
            Locale::Pt => "Alias não encontrado ou você não tem permissão para usá-lo.",
        }
    }

    fn dkim_modal_title(&self, domain: &str) -> String {
        match self {
            Locale::En => format!("DKIM Configuration — {}", domain),
            Locale::Es => format!("Configuración de DKIM — {}", domain),
            Locale::Fr => format!("Configuration DKIM — {}", domain),
            Locale::Pt => format!("Configuração DKIM — {}", domain),
        }
    }

    fn dkim_no_key_warning(&self) -> &'static str {
        match self {
            Locale::En => "No DKIM Signature Key Configured",
            Locale::Es => "Sin Clave de Firma DKIM Configurada",
            Locale::Fr => "Aucune clé de signature DKIM configurée",
            Locale::Pt => "Nenhuma Chave de Assinatura DKIM Configurada",
        }
    }

    fn dkim_no_key_desc(&self) -> &'static str {
        match self {
            Locale::En => {
                "This domain was created without DKIM keys. Generating a DKIM key pair is highly recommended to authenticate outgoing emails and ensure high inbox deliverability."
            }
            Locale::Es => {
                "Este dominio fue creado sin claves DKIM. Se recomienda encarecidamente generar un par de claves DKIM para autenticar los correos salientes y garantizar una alta entrega en la bandeja de entrada."
            }
            Locale::Fr => {
                "Ce domaine a été créé sans clés DKIM. La génération d'une paire de clés DKIM est fortement recommandée pour authentifier les e-mails sortants et garantir une délivrabilité élevée."
            }
            Locale::Pt => {
                "Este domínio foi criado sem chaves DKIM. Recomenda-se fortemente a geração de um par de chaves DKIM para autenticar e-mails de saída e garantir uma alta entrega na caixa de entrada."
            }
        }
    }

    fn dkim_generate_button(&self) -> &'static str {
        match self {
            Locale::En => "Generate DKIM Key",
            Locale::Es => "Generar Clave DKIM",
            Locale::Fr => "Générer la clé DKIM",
            Locale::Pt => "Gerar Chave DKIM",
        }
    }

    fn dkim_pending_title(&self) -> &'static str {
        match self {
            Locale::En => "Pending DKIM Key Verification",
            Locale::Es => "Verificación de Clave DKIM Pendiente",
            Locale::Fr => "Vérification de la clé DKIM en attente",
            Locale::Pt => "Verificação de Chave DKIM Pendente",
        }
    }

    fn dkim_rotation_active_badge(&self) -> &'static str {
        match self {
            Locale::En => "Rotation Active",
            Locale::Es => "Rotación Activa",
            Locale::Fr => "Rotation active",
            Locale::Pt => "Rotação Ativa",
        }
    }

    fn dkim_pending_desc(&self) -> &'static str {
        match self {
            Locale::En => {
                "To complete key rotation, publish this new TXT record. Old record must remain active in parallel!"
            }
            Locale::Es => {
                "Para completar la rotación de la clave, publique este nuevo registro TXT. ¡El registro anterior debe permanecer activo en paralelo!"
            }
            Locale::Fr => {
                "Pour terminer la rotation des clés, publiez ce nouvel enregistrement TXT. L'ancien enregistrement doit rester actif en parallèle !"
            }
            Locale::Pt => {
                "Para concluir a rotação da chave, publique este novo registo TXT. O registo antigo deve permanecer ativo em paralelo!"
            }
        }
    }

    fn dkim_active_title(&self) -> &'static str {
        match self {
            Locale::En => "Active DKIM Configuration",
            Locale::Es => "Configuración DKIM Activa",
            Locale::Fr => "Configuration DKIM active",
            Locale::Pt => "Configuração DKIM Ativa",
        }
    }

    fn dkim_active_desc(&self) -> &'static str {
        match self {
            Locale::En => {
                "Publish this TXT record on your domain's DNS management console (e.g. Cloudflare, Route53, GoDaddy):"
            }
            Locale::Es => {
                "Publique este registro TXT en la consola de administración de DNS de su dominio (por ejemplo, Cloudflare, Route53, GoDaddy):"
            }
            Locale::Fr => {
                "Publiez cet enregistrement TXT sur la console de gestion DNS de votre domaine (ex. Cloudflare, Route53, GoDaddy) :"
            }
            Locale::Pt => {
                "Publique este registo TXT na consola de administração de DNS do seu domínio (por exemplo, Cloudflare, Route53, GoDaddy):"
            }
        }
    }

    fn dkim_rotate_button(&self) -> &'static str {
        match self {
            Locale::En => "Rotate DKIM Key",
            Locale::Es => "Rotar Clave DKIM",
            Locale::Fr => "Faire pivote la clé DKIM",
            Locale::Pt => "Rodar Chave DKIM",
        }
    }

    fn dkim_rotate_confirm(&self) -> &'static str {
        match self {
            Locale::En => {
                "Rotating your DKIM key will generate a new key pair and selector. Your current active key will remain active until you verify the new one. Do you want to proceed?"
            }
            Locale::Es => {
                "Rotar su clave DKIM generará un nuevo par de claves y selector. Su clave activa actual permanecerá activa hasta que verifique la nueva. ¿Desea continuar?"
            }
            Locale::Fr => {
                "La rotation de votre clé DKIM générera une nouvelle paire de clés et un nouveau sélecteur. Votre clé active actuelle restera active jusqu'à ce que vous vérifiiez la nouvelle. Voulez-vous continuer ?"
            }
            Locale::Pt => {
                "Rodar a sua chave DKIM irá gerar um novo par de chaves e seletor. A sua chave ativa atual permanecerá ativa até verificar a nova. Deseja continuar?"
            }
        }
    }

    fn dkim_type_label(&self) -> &'static str {
        match self {
            Locale::En => "Type",
            Locale::Es => "Tipo",
            Locale::Fr => "Type",
            Locale::Pt => "Tipo",
        }
    }

    fn dkim_host_label(&self) -> &'static str {
        match self {
            Locale::En => "Host",
            Locale::Es => "Host",
            Locale::Fr => "Hôte",
            Locale::Pt => "Host",
        }
    }

    fn dkim_value_label(&self) -> &'static str {
        match self {
            Locale::En => "Value",
            Locale::Es => "Valor",
            Locale::Fr => "Valeur",
            Locale::Pt => "Valor",
        }
    }

    fn dkim_copy_button(&self) -> &'static str {
        match self {
            Locale::En => "Copy",
            Locale::Es => "Copiar",
            Locale::Fr => "Copier",
            Locale::Pt => "Copiar",
        }
    }

    fn dkim_ttl_notice(&self) -> &'static str {
        match self {
            Locale::En => {
                "DNS updates can take time to propagate. If verification fails initially, please check your registrar and try again shortly."
            }
            Locale::Es => {
                "Las actualizaciones de DNS pueden tardar en propagarse. Si la verificación falla inicialmente, verifique su proveedor e inténtelo de nuevo en unos minutos."
            }
            Locale::Fr => {
                "Les mises à jour DNS peuvent prendre du temps à se propager. Si la vérification échoue initialement, veuillez vérifier votre bureau d'enregistrement et réessayer sous peu."
            }
            Locale::Pt => {
                "As atualizações de DNS podem demorar a propagar-se. Se a verificação falhar inicialmente, verifique o seu registador e tente novamente em breve."
            }
        }
    }

    fn dkim_verify_button(&self) -> &'static str {
        match self {
            Locale::En => "Verify & Activate",
            Locale::Es => "Verificar y Activar",
            Locale::Fr => "Vérifier & Activer",
            Locale::Pt => "Verificar e Ativar",
        }
    }

    fn dkim_cancel_button(&self) -> &'static str {
        match self {
            Locale::En => "Cancel Rotation",
            Locale::Es => "Cancelar Rotación",
            Locale::Fr => "Annuler la rotation",
            Locale::Pt => "Cancelar Rotação",
        }
    }

    fn dkim_cancel_confirm(&self) -> &'static str {
        match self {
            Locale::En => {
                "Are you sure you want to cancel the pending DKIM rotation? Any DNS record you published for this selector will no longer be used."
            }
            Locale::Es => {
                "¿Está seguro de que desea cancelar la rotación de DKIM pendiente? Cualquier registro DNS que haya publicado para este selector ya no se utilizará."
            }
            Locale::Fr => {
                "Êtes-vous sûr de vouloir annuler la rotation DKIM en attente ? Tout enregistrement DNS que vous avez publié pour ce sélecteur ne sera plus utilisé."
            }
            Locale::Pt => {
                "Tem a certeza de que deseja cancelar a rotação de DKIM pendente? Qualquer registo DNS que tenha publicado para este seletor deixará de ser utilizado."
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    #[tokio::test]
    async fn test_locale_extraction() {
        async fn get_locale(lang_header: Option<&str>) -> Locale {
            let mut request = Request::builder().uri("/");
            if let Some(h) = lang_header {
                request = request.header("accept-language", h);
            }
            let req = request.body(()).unwrap();
            let (mut parts, _) = req.into_parts();

            Locale::from_request_parts(&mut parts, &()).await.unwrap()
        }

        // Test Spanish
        assert!(matches!(
            get_locale(Some("es-ES,es;q=0.9")).await,
            Locale::Es
        ));
        assert!(matches!(get_locale(Some("ES")).await, Locale::Es));

        // Test French
        assert!(matches!(
            get_locale(Some("fr-FR,fr;q=0.9")).await,
            Locale::Fr
        ));
        assert!(matches!(get_locale(Some("FR")).await, Locale::Fr));

        // Test Portuguese
        assert!(matches!(
            get_locale(Some("pt-PT,pt;q=0.9")).await,
            Locale::Pt
        ));
        assert!(matches!(get_locale(Some("PT")).await, Locale::Pt));

        // Test English (Default)
        assert!(matches!(
            get_locale(Some("en-US,en;q=0.9")).await,
            Locale::En
        ));

        // Test Missing Header (Default to English)
        assert!(matches!(get_locale(None).await, Locale::En));

        // Test Other Language (Default to English)
        assert!(matches!(get_locale(Some("de-DE")).await, Locale::En));
    }

    #[test]
    fn test_messages_content() {
        let locales = [Locale::En, Locale::Es, Locale::Fr, Locale::Pt];

        for locale in locales {
            // Check that strings are not empty
            assert!(!locale.login_title().is_empty());
            assert!(!locale.modal_cancel().is_empty());
            assert!(!locale.modal_delete_confirm().is_empty());

            // Parameterized messages
            let alias_msg = locale.delete_alias_message("test@alias.com");
            assert!(alias_msg.contains("test@alias.com"));

            let email_msg = locale.delete_email_message("Important Subject");
            assert!(email_msg.contains("Important&#32;Subject"));

            let limit_msg = locale.limit_reached_text(&5);
            assert!(limit_msg.contains("5"));
        }

        // Ensure uniqueness across languages for key strings
        assert_ne!(Locale::En.login_title(), Locale::Es.login_title());
        assert_ne!(Locale::En.login_title(), Locale::Fr.login_title());
        assert_ne!(Locale::En.login_title(), Locale::Pt.login_title());

        assert_ne!(Locale::En.modal_cancel(), Locale::Es.modal_cancel());
        assert_ne!(Locale::En.modal_cancel(), Locale::Fr.modal_cancel());
        assert_ne!(Locale::En.modal_cancel(), Locale::Pt.modal_cancel());
    }

    #[test]
    fn test_xss_sanitization_in_delete_messages() {
        let locale = Locale::En;

        let malicious_alias = "<script>alert('alias')</script>";
        let safe_alias_msg = locale.delete_alias_message(malicious_alias);
        assert!(!safe_alias_msg.contains("<script>"));
        assert!(
            safe_alias_msg.contains("&lt;script&gt;alert(&apos;alias&apos;)&lt;&#47;script&gt;")
        );

        let malicious_subject = "<b>Free money</b> & <img src=x onerror=alert(1)>";
        let safe_subject_msg = locale.delete_email_message(malicious_subject);
        assert!(!safe_subject_msg.contains("<b>"));
        assert!(!safe_subject_msg.contains("<img"));
        assert!(safe_subject_msg.contains("&lt;b&gt;Free&#32;money&lt;&#47;b&gt;&#32;&amp;&#32;&lt;img&#32;src&#61;x&#32;onerror&#61;alert(1)&gt;"));
    }
}
