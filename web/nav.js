document.addEventListener('DOMContentLoaded', function() {
    const nav = document.createElement('nav');
    nav.innerHTML = `
        <a href="/" class="nav-title">ferros</a>
        <ul>
            <li><a href="https://holdmyoscilloscope.com/projects/">PROJECTS</a></li>
            <li><a href="https://holdmyoscilloscope.com/about/">ABOUT</a></li>
            <li><a href="https://holdmyoscilloscope.com/blog/">BLOG</a></li>
            <li><a href="https://holdmyoscilloscope.com/art/">ART</a></li>
        </ul>
    `;
    document.body.insertBefore(nav, document.body.firstChild);
});
