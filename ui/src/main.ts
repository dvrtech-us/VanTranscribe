import { mount } from 'svelte';
import 'bootstrap/dist/css/bootstrap.min.css';
import './style.css';
import App from './App.svelte';

mount(App, { target: document.getElementById('app')! });
