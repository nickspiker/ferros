document.addEventListener('DOMContentLoaded', function() {
    const footer = document.createElement('footer');
    footer.innerHTML = `
        <p>&copy; Nick Spiker</p>
        <p class="tagline">engineering excellence in the signal domain</p>
    `;
    document.body.appendChild(footer);
});
