const yearElement = document.getElementById("year");
const epicLoginButton = document.getElementById("epic-login");

if (yearElement) {
  yearElement.textContent = String(new Date().getFullYear());
}

epicLoginButton?.addEventListener("click", () => {
  // This route must be implemented by the Rust backend before sign-in can work.
  window.location.assign("/auth/epic/login");
});
