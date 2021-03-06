// Copyright (c) 2017-2019, Substratum LLC (https://substratum.net) and/or its affiliates. All rights reserved.

import {async, ComponentFixture, TestBed} from '@angular/core/testing';
import {NodeConfigurationComponent} from './node-configuration.component';
import {func, reset, verify, when} from 'testdouble';
import {ConfigService} from '../config.service';
import {FormsModule, ReactiveFormsModule} from '@angular/forms';
import {Router} from '@angular/router';
import {BehaviorSubject, Subject} from 'rxjs';
import {NodeConfiguration} from '../node-configuration';
import {NodeConfigurationPage} from './node-configuration-page';

describe('NodeConfigurationComponent', () => {
  let component: NodeConfigurationComponent;
  let fixture: ComponentFixture<NodeConfigurationComponent>;
  let mockConfigService;
  let page: NodeConfigurationPage;
  let mockRouter;
  let mockSave;
  let mockLoad;
  let mockNavigateByUrl;
  let storedConfig: Subject<NodeConfiguration>;

  beforeEach(async(() => {
    storedConfig = new BehaviorSubject(new NodeConfiguration());
    mockSave = func('save');
    mockLoad = func('load');
    mockNavigateByUrl = func('navigateByUrl');

    mockConfigService = {
      save: mockSave,
      load: mockLoad
    };
    mockRouter = {
      navigateByUrl: mockNavigateByUrl
    };
    TestBed.configureTestingModule({
      declarations: [NodeConfigurationComponent],
      imports: [
        FormsModule,
        ReactiveFormsModule
      ],
      providers: [
        {provide: ConfigService, useValue: mockConfigService},
        {provide: Router, useValue: mockRouter}
      ]
    })
      .compileComponents();
  }));

  beforeEach(() => {
    when(mockConfigService.load()).thenReturn(storedConfig.asObservable());
    fixture = TestBed.createComponent(NodeConfigurationComponent);
    page = new NodeConfigurationPage(fixture);
    component = fixture.componentInstance;
    fixture.detectChanges();
  });

  afterEach(() => {
    reset();
  });

  it('should create', () => {
    expect(component).toBeTruthy();
  });

  describe('Wallet section', () => {
    xdescribe('when toggled to consuming', () => {
      beforeEach(() => {
        page.consumingWalletBtn.click();
        fixture.detectChanges();
      });

      it('consuming is highlighted to indicate it is selected', () => {
        expect(page.consumingWalletBtn.classList).toContain('button-active');
        expect(page.earningWalletBtn.classList).not.toContain('button-active');
      });

      it('allows the user to enter a private key', () => {
        expect(page.privateKeyTxt).toBeTruthy();
      });
    });

    describe('when toggled to earning', () => {
      beforeEach(() => {
        page.earningWalletBtn.click();
        fixture.detectChanges();
      });

      it('earning is highlighted to indicate it is selected', () => {
        expect(page.earningWalletBtn.classList).toContain('button-active');
        // Re-enable when consuming is shown
        // expect(page.consumingWalletBtn.classList).not.toContain('button-active');
      });

      it('only allows the user to enter a wallet address', () => {
        expect(page.privateKeyTxt).toBeFalsy();
      });
    });

    describe('with a filled out form', () => {
      describe('when submitted', () => {
        const expected = {
          ip: '127.0.0.1',
          neighbor: 'neighbornodedescriptor',
          walletAddress: 'address',
          // privateKey: 'private',
          privateKey: '',
        };

        beforeEach(() => {
          page.setIp('127.0.0.1');
          page.setNeighbor('neighbornodedescriptor');
          // page.setPrivateKey('private');
          page.setWalletAddress('address');
          page.saveConfigBtn.click();
          fixture.detectChanges();
        });

        it('persists the values', () => {
          verify(mockSave(expected));
        });

        it('redirects to the main screen', () => {
          verify(mockNavigateByUrl('/index'));
        });
      });
    });
  });

  describe('when configuration already exists', () => {
    const expected: NodeConfiguration = {
      ip: '127.0.0.1',
      neighbor: 'neighbornodedescriptor',
      walletAddress: 'address',
      // privateKey: 'private', switch back when consuming is brought in
      privateKey: '',
    };

    beforeEach(() => {
      storedConfig.next(expected);
    });

    it('is prepopulated with that data', () => {
      expect(page.ipTxt.value).toBe('127.0.0.1');
      expect(page.neighborTxt.value).toBe('neighbornodedescriptor');
      // expect(page.privateKeyTxt.value).toBe('private');
      expect(page.walletAddressTxt.value).toBe('address');
    });
  });

  describe('when clicking the node descriptor help icon', () => {

    beforeEach(() => {
      page.nodeDescriptorHelpImg.click();
      fixture.detectChanges();
    });

    it('displays the help message', () => {
      expect(page.nodeDescriptorTooltip).toBeTruthy();
    });

    describe('clicking anywhere', () => {
      beforeEach(() => {
        expect(page.nodeDescriptorTooltip).toBeTruthy();
        page.containerDiv.click();
        fixture.detectChanges();
      });

      it('hides the help message', () => {
        expect(page.nodeDescriptorTooltip).toBeFalsy();
      });
    });
  });
});
